# Glossary

These terms describe the current [event model](../src/lib.rs), [local log](architecture/storage.md), and [delivery design](architecture/delivery.md). Terms that still require a protocol are marked as proposed.

| Term | Meaning here |
| --- | --- |
| Event | One claim about a resource, attributed to a source, with identity, times, context, and a payload. Represented by `Event`. |
| Event ID | An identifier assigned by the producer of an event. The synthetic generator uses IDs starting at 1; `EventId(u64)` does not enforce uniqueness across producers. |
| Source | The emitter or observer of an event. `SourceId` is a reference; there is no source registry or collector implementation. |
| Resource | The thing an event describes, such as a process or service. `ResourceId` can refer to a different thing from the source. |
| Tenant | The logical owner associated with the event. `TenantId` currently supplies context; it does not implement access control or quotas. |
| Event time | When the described event happened. `EventTime(i64)` is documented in Unix nanoseconds. |
| Observed time | When the source observed the event. `ObservedTime(i64)` is distinct from event time and uses the same documented unit. |
| Attribute | A string key and a typed `Scalar` value, carried as context in a vector. Duplicate keys are currently possible. |
| Scalar | One of `Bool`, `I64`, `U64`, `F64`, or `String`, used as an attribute value. |
| Payload | The signal-specific content, selected by the `Payload` enum. |
| Log | The `Payload::Log` variant, containing a string body. No severity or log ingestion behavior exists yet. |
| Gauge | The `Payload::Gauge` variant: a named floating-point measurement with a string unit. No aggregation or unit validation exists yet. |
| Workload config | The generator's `seed: u64` and `events: u32` settings. They determine the sequence emitted by this version of the generator. |
| Event generator | An iterator that builds one owned synthetic Gauge event per call to `next`, until it has emitted the configured count. It is not a collector of real telemetry. |
| Event buffer | The local, single-threaded `EventBuffer` that holds at most a configured positive number of events in FIFO order. Its event-count bound is not a byte bound. |
| Batch | An owned `Vec<Event>` removed from the front of the buffer, containing up to the requested positive number of events. The final batch may be shorter. |
| Rejection | A full-buffer `try_push` result of `Err(event)` that gives the same owned event back to the caller without changing the queue. It does not mean the event was dropped. |
| Event log | The local append-only file of framed `Event` records managed by `EventLog`. It is separate from the volatile event buffer. |
| Frame | One versioned event header and encoded event payload in the log. It is replayable only when followed by a valid commit marker. |
| Commit marker | A checked 16-byte record written after the event frame has been synced; it marks which event the log may replay. |
| Local commit | `EventLog::append` returning successfully after syncing the event frame and its commit marker. This is a receiver-side durability boundary under the stated filesystem assumptions. |
| Upstream copy (target protocol) | A sender-held copy retained so the event can be sent again until a durable receiver acknowledges it. The current CLI reconstructs a synthetic sequence from seed/count rather than persisting an independent sender copy. |
| Volatile receiver copy | An accepted in-memory copy that the receiver can lose on crash. `EventBuffer` holds such values but does not by itself establish a durable owner. |
| Durable owner | A receiver with a committed record that can be replayed after the modeled crash. The local log provides this under its filesystem assumptions; this is not a network delivery guarantee. |
| Acknowledgement, ACK (target protocol) | A confirmation received by a sender after durable commit. Only then may it discard its retryable copy. The CLI prints a local commit line but sends no ACK. |
| Receiver credit (H1 simulation) | Permission for one scheduled data packet in the [packet-slot model](experiments/ablation/receiver-credit-h1-run-01.md). Issuing or consuming CREDIT does not acknowledge durable storage. |
| Unscheduled prefix (M2 simulation) | The first DATA packet of a message sent before receiver credit. It carries at most 1,468 payload bytes plus a 32-byte announcement in one 1,500-byte wire slot. The receiver can grant the remaining packets only after that first packet is delivered. See the [M2 comparison](experiments/ablation/unscheduled-prefix-m2-run-01.md). |
| Packet receipt ACK (H1 simulation) | A flow-control signal that frees one M0 sender-window slot after a packet reaches the modeled receiver. It is separate from the modeled post-commit durable ACK for the whole message. |
| Stable message identity (transport research) | The pair `(producer, sequence)` used by the synthetic transport trace and finite model for deduplication questions. The application has no corresponding network identity or deduplication implementation yet. |
| Snapshot anchor (coverage research) | An independently retained root, snapshot identity, block count and row count used to verify block commitments. It is not a persisted application commit. |
| Coverage receipt (coverage research) | Query-bound metadata accounting for each snapshot block as scanned, safely excluded or unavailable. Verification distinguishes complete, incomplete and invalid receipts under the [stated trust assumptions](architecture/query.md). |

An architecture decision record (ADR) records a consequential choice and its evidence. An ablation is an experiment that removes or changes a mechanism while keeping the compared contract and workload explicit. Neither a proposed experiment nor a diagram is proof that a property has been checked.
