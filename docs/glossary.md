# Glossary

Terms mean one thing each across code, documents, diagrams and experiments. When a term changes meaning, update this page and the affected pages together. Two themed terms are canonical, Spindle and Strand ([ADR-0017](decisions/ADR-0017-name-the-spindle-and-the-strand.md)); everything else has a functional name.

## Product

The [operator console](milestones/operator-console.md) uses a **display envelope**:
bounded per-time-bucket extrema with explicit gap markers, not an aggregate or
exhaustive raw history. A **display eviction** removes an older row from bounded
browser memory; it is not server retention or evidence of collection loss.
A **principal** in the intended [access design](architecture/identity-access.md)
is a persistent human, workload or Spindle identity; a credential authenticates
that principal, and a grant limits its permitted actions/resources.

| Term | Meaning here |
| --- | --- |
| Spindle | The host-resident collection and runtime role: observes one host, reads configured sources, builds Batches, keeps collection state, retains unacknowledged Batches in its Spool, applies validated configuration and delivers to Fabric Server. Run by the `fabric-node` executable; code in `src/spindle`. Not a synonym for any process, worker or remote machine. |
| Spindle ID | `SpindleId`: the Spindle's stable 16-byte identity. On the wire and on disk it is the envelope's `node_id` field. |
| Node (persisted name) | The spelling used by existing routes (`/v1/admin/nodes`), JSON keys, configuration files, the `fabric-node` binary and systemd unit. It names a Spindle or its enrollment; it is kept for compatibility, not as a separate concept. |
| Generation | A non-zero counter in the Spindle identity file. A new generation starts a new Strand at sequence 1. |
| Strand | One ordered telemetry lineage of one Spindle generation: `StrandId = (SpindleId, generation)`. The scope of sequence ordering, duplicate-retry identity, gap detection, deduplication and acknowledged progress. Not a connection, session, thread, file, Segment or query result. Called a "stream" in older code and records. |
| Sequence | A Batch's position on its Strand: 1, 2, 3, ... Sequence 0 does not exist; no sequence follows `u64::MAX`. |
| Batch | The version-one Fabric envelope: Strand identity, sequence, exact encoded OTLP metrics and logs bytes, source cursors and collection gaps (`fabric_frame::envelope::Batch`). Retries send its exact stored bytes. |
| Spool | The Spindle's durable `FAB1` frame log of Batches with an ACK cursor; whole sealed files at or below the cursor are reclaimed. |
| Delivery | Transfer of custody of a Batch from the Spool to the server: one Batch in flight per Strand, oldest unacknowledged first ([ADR-0013](decisions/ADR-0013-deliver-batches-in-order-with-bounded-dedup.md)). |
| Custody | Responsibility for preserving telemetry. The Spindle holds it until an ACK that follows the server's durable commit. |
| ACK | The server's answer `ack` with `committed_through`, sent only after the group holding the Batch (or the earlier Batch it acknowledges) completed data and marker syncs. |
| Server journal | The server's `FAB1` frame log of committed groups of Batches, replayed to rebuild Strand and binding state. |
| Segment | An immutable directory of Zstd Parquet files plus a manifest written last, covering a contiguous range of journal groups. |
| Observation | The proposed one record for a log line, a metric point or a span: the query key `(time_ns, node_id, generation, sequence, index)`, optional trace locators, typed attributes and one signal payload ([ADR-0023](decisions/ADR-0023-define-an-observation-record-with-a-canonical-encoding.md)). Not yet emitted or stored. |
| FOB1 | The Observation's canonical block encoding: dictionaries in first-use order, delta-coded columns, a CRC trailer; one byte string per valid block and one block per accepted byte string, so a hash of the bytes is a hash of the records. |
| Canonical encoding | An encoding in which `decode(encode(b)) == b` for every valid value and `encode(decode(x)) == x` for every accepted byte string; the decoder rejects every other form. |
| Sealer | The server's background thread that turns each sealed journal file into a Segment, then lets the commit thread delete the file, and applies retention. It runs after ingestion and never sits on the ACK path. |
| Run | A sorted batch of log rows or metric points, spilled to a scratch file while a Segment is built. Runs are merged into the final table and deleted before the Segment commits; they are not a durable format. |
| Retention | Deleting whole Segments, oldest first, when the age or byte limit is exceeded; the retained window is reported with every answer. |
| Snapshot | The group range a query answer was computed from; a page token binds it. |
| Completeness | Whether every Segment and journal file that could hold matching rows was read and verified (`complete`), with the unavailable ones listed. |
| Freshness | Per Spindle, the newest observation or point time retained, reported with every answer. |
| Collection gap | A recorded interval or source failure during which telemetry was not collected, carried in a Batch; never silently omitted. |
| Control | Enrollment, desired and applied configuration revisions, pause, resume and revoke ([ADR-0014](decisions/ADR-0014-manage-nodes-through-server-control-state.md)). |

## Architecture

| Term | Meaning here |
| --- | --- |
| Core | `fabric-core`: pure, deterministic, `no_std` domain decisions over explicit inputs; effects are returned as data ([ADR-0016](decisions/ADR-0016-keep-a-pure-semantic-core.md)). |
| Port | An effect contract an application use case needs, such as `DurableJournal` or `Clock` (`fabric-ports`). A port is an effect boundary, not a pure function. |
| Application use case | Orchestration in `fabric-app` that calls core decisions and ports, never a concrete adapter. |
| Adapter | An implementation of a port that performs effects: Linux, filesystem, HTTP/TLS, Parquet, clock. |
| Adapter support | Infrastructure shared by adapters that is not domain semantics, such as the `FAB1` frame log and the envelope codec (`fabric-frame`). |
| Composition root | A binary or library that wires adapters into use cases; today `fabric-server` and the root package, which still contain adapters. |
| Query plan | How a history query reads its sources, set by the server key `query_plan`: `scan` decodes every tail entry and reads every row group the window admits; `walk` reads sources in order of the smallest key each can hold and stops at the heap's threshold ([ADR-0024](decisions/ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md)). Both return the same answer. |
| Trace endpoint | The Spindle's loopback OTLP/HTTP `POST /v1/traces` receiver (`traces_listen`), which commits each export to the Spool before answering ([ADR-0025](decisions/ADR-0025-carry-traces-as-a-third-signal.md)). |
| Output meter | The Spindle's counters of what it commits and delivers, reported as its own `fabric.spindle.*` metrics, and its optional delivery rate cap (`max_output_bytes_per_s`). |
| Span row | One OTLP span as a query row: node, Strand position, hex trace, span and parent IDs, name, kind, status, start and end times, string attributes. |
| Text filter | `text_filter.bin` in a Segment: one Bloom filter per `logs` row group over the distinct byte trigrams of its bodies, written by the sealer, used by the walk plan only after its digest matches the manifest ([ADR-0024](decisions/ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md) part 2). An optional index: missing or corrupt, the walk reads exactly. |
| Tail block | A FOB1 block of 4,096 tail records with a trigram filter, derived in memory from the journal by the walk plan and read in place of decoding its entries; never persisted ([ADR-0024](decisions/ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md) part 3). |
| Tail index | The walk plan's per-process record of every unsealed journal entry: group, frame position, node, receive time and the time bounds of its rows, without the rows. |
| Layer gate | `cargo xtask check-layers` over [layers.json](architecture/layers.json). |
| Purity gate | `cargo xtask check-core-purity` over [core-purity.json](architecture/core-purity.json). |

## Verification

| Term | Meaning here |
| --- | --- |
| Independent oracle | A checker implemented separately from the product (here in Python, frozen before the code it grades) that decides correctness of observed behavior. A test generated with an implementation is not one. |
| Negative control | A representative defect a checker must reject, proving it can fail. |
| Semantic mutant | A hand-written incorrect variant of product code tied to one contract and one named checker ([xtask/mutants.json](../xtask/mutants.json)). |
| Counterexample fixture | A minimized failing case kept as a deterministic regression with its origin and fix. |
| Trust-boundary change | A change to the product contract, an oracle, a negative-control expectation, a formal invariant, a registered protocol or verification policy; made in its own commit. |
| Verification receipt | The JSON record `cargo xtask checks` writes per check; unsigned, so it proves structure, not execution. |
| Evidence states | Implemented, Tested, Measured, Qualified, Not run, Interrupted, Failed, Inconclusive ([qualification](QUALIFICATION.md#evidence-states)). |
| Milestone | A stable engineering objective with a `milestone/<capability>` branch ([roadmap](ROADMAP.md)). Release maturity is a tag, not a milestone. |

## FOL2 demonstration and research

These terms describe the original [event model](../src/lib.rs), [local log](architecture/storage.md), the [delivery design](architecture/delivery.md) and research tooling. Terms that still require a protocol are marked as proposed.

| Term | Meaning here |
| --- | --- |
| Event | One claim about a resource, attributed to a source, with identity, times, context, and a payload. Represented by `Event`. |
| Event ID | An identifier assigned by the producer of an event. The synthetic generator uses IDs starting at 1; `EventId(u64)` does not enforce uniqueness across producers. |
| Source | The emitter or observer of an event. `SourceId` is a reference; there is no source registry. The research CLI has a restricted offline Logs file adapter, not a network collector. |
| Resource | The thing an event describes, such as a process or service. `ResourceId` can refer to a different thing from the source. |
| Tenant | The logical owner associated with the event. `TenantId` currently supplies context; it does not implement access control or quotas. |
| Event time | When the described event happened. `EventTime(i64)` is documented in Unix nanoseconds. |
| Observed time | When the source observed the event. `ObservedTime(i64)` is distinct from event time and uses the same documented unit. |
| Attribute | A string key and a typed `Scalar` value, carried as context in a vector. Duplicate keys are currently possible. |
| Scalar | One of `Bool`, `I64`, `U64`, `F64`, or `String`, used as an attribute value. |
| Payload | The signal-specific content, selected by the `Payload` enum. |
| Log | The `Payload::Log` variant, containing a string body. The research adapter maps selected OTLP/JSON Logs metadata to attributes; the application has no log receiver. |
| Gauge | The `Payload::Gauge` variant: a named floating-point measurement with a string unit. No aggregation or unit validation exists yet. |
| Workload config | The generator's `seed: u64` and `events: u32` settings. They determine the sequence emitted by this version of the generator. |
| Event generator | An iterator that builds one owned synthetic Gauge event per call to `next`, until it has emitted the configured count. It is not a collector of real telemetry. |
| Event buffer | The local, single-threaded `EventBuffer` that holds at most a configured positive number of events in FIFO order. Its event-count bound is not a byte bound. |
| Event batch (demo) | An owned `Vec<Event>` removed from the front of the demo buffer, containing up to the requested positive number of events. Distinct from a product Batch. |
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
| Publication (research prototype) | The caller-retained S2 snapshot anchor, block size and codec version, stored outside the snapshot directory as an independent query trust input. |
| Checkpoint (research prototype) | A fresh file of validated query pages; its SHA-256 digest must also be retained independently for resume. It is bound to one snapshot root, query and semantic versions. |
| Residual (research prototype) | The unresolved block ordinals derived from a validated checkpoint. A resume reads only candidate work for the same root. |
| Cold copy (research prototype) | A local `cold/` copy of a snapshot block that may satisfy a missing or unreadable hot block after authentication. It does not imply remote object storage. |
| Hybrid layout (layout research) | An Arrow/Parquet table with predicate projection columns and a duplicate complete raw Event column. Whole-file authentication precedes projected queries. |

An architecture decision record (ADR) records a consequential choice and its evidence. An ablation is an experiment that removes or changes a mechanism while keeping the compared contract and workload explicit. Neither a proposed experiment nor a diagram is proof that a property has been checked.
