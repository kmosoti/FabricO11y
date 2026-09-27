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

This first record captures an existing, explicitly documented choice. It does not claim that storage engines, ID widths, or performance strategies have been compared. Record further decisions when they materially affect architecture; use the [ADR format in AGENTS.md](../../AGENTS.md#12-architecture-decision-records) and the next unused four-digit number.

Return to the [architecture documentation](../README.md).
