# Architecture decisions

| Record | Status | Scope |
| --- | --- | --- |
| [ADR-0001: Keep the event domain independent of infrastructure](ADR-0001-keep-domain-independent.md) | Accepted for the initial prototype | Existing library boundary |
| [ADR-0002: Return an event when the local buffer is full](ADR-0002-reject-full-buffer.md) | Accepted for Stage 3 | Full-buffer ownership and FIFO batching |
| [ADR-0003: Observe agent activity with typed events](ADR-0003-observe-agent-activity-with-typed-events.md) | Accepted for the first passive increment | Agent telemetry and model-context boundary |
| [ADR-0004: Use `fake` for synthetic workload values](ADR-0004-use-fake-for-synthetic-values.md) | Accepted for the local workload | Seeded generation dependency and Fabric adapter |
| [ADR-0005: Acknowledge only after durable commit](ADR-0005-ack-after-durable-commit.md) | Accepted; local receiver commit implemented | Delivery ownership and ACK ordering |
| [ADR-0006: Use a framed local event log](ADR-0006-use-framed-local-log.md) | Accepted for Stage 5 | Storage format, commit, and recovery |
| [ADR-0007: Experiment with authenticated block coverage](ADR-0007-experiment-with-coverage-receipts.md) | Experimental | Research-only trusted builder, snapshot anchor and metadata verification |
| [ADR-0008: Bind residual answers to immutable snapshots](ADR-0008-bind-residuals-to-immutable-snapshots.md) | Experimental | Research retry binding, physical identity and atomic per-block joins |
| [ADR-0009: Isolate durable research snapshots](ADR-0009-isolate-durable-research-snapshots.md) | Experimental | Fresh immutable publication, external trust and checkpoint history |
| [ADR-0010: Use static systemd services for the Linux alpha](ADR-0010-use-static-systemd-services-for-alpha.md) | Accepted design, pending installation | Static `fabricolly` identity, two services and one aggregate slice |
| [ADR-0011: Separate an interrupted append from a known journal failure](ADR-0011-separate-interrupted-append-from-known-failure.md) | Accepted | Alpha `FAB1` reopen after process death; known failures still refuse |
| [ADR-0012: Add the Fabric Server as a workspace crate](ADR-0012-add-a-server-crate-in-a-workspace.md) | Accepted | Workspace member, synchronous node client, ring provider, no SQLite |
| [ADR-0013: Deliver batches in order with bounded dedup](ADR-0013-deliver-batches-in-order-with-bounded-dedup.md) | Accepted | One in flight per stream, per-stream last sequence and hash, grouped two-sync commits |
| [ADR-0014: Manage nodes through server control state and node polling](ADR-0014-manage-nodes-through-server-control-state.md) | Accepted | Admin token, enrollment, desired and applied revisions, 5 s polls |
| [ADR-0015: Adopt a hexagonal architecture with a checked dependency rule](ADR-0015-adopt-a-hexagonal-architecture.md) | Accepted | Core, ports, app, adapter support, adapters, composition roots; `cargo xtask check-layers` |
| [ADR-0016: Keep the semantic core pure, deterministic and `no_std`](ADR-0016-keep-a-pure-semantic-core.md) | Accepted | Explicit inputs, effects as data, `no_std`, `cargo xtask check-core-purity` |
| [ADR-0017: Name the Spindle and the Strand, and nothing else thematically](ADR-0017-name-the-spindle-and-the-strand.md) | Accepted | Two canonical themed terms; wire names and the `fabric-node` binary unchanged |
| [ADR-0018: Accept work on executable evidence, not on model review](ADR-0018-accept-work-on-executable-evidence.md) | Accepted | Independent oracles, negative controls, trust-boundary changes, counterexample fixtures, receipts; no model-review gate |
| [ADR-0019: Keep release maturity in tags, not in names](ADR-0019-keep-release-maturity-in-tags.md) | Accepted | No release-stage namespaces; historical evidence keeps its words |

This first record captures an existing, explicitly documented choice. It does not claim that storage engines, ID widths, or performance strategies have been compared. Record further decisions when they materially affect architecture; use the [ADR format](../documentation-policy.md#12-architecture-decision-records) and the next unused four-digit number.

Return to the [architecture documentation](../README.md).
