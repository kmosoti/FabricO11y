# Architecture views

Current product views use the [evidence states](../QUALIFICATION.md#evidence-states).

| Area | View | Scope and status |
| --- | --- | --- |
| Runtime and boundaries | [System](system.md) | Runtime components, crate layers and dependency rule. |
| Collection | [Spindle](spindle.md) | Host collection, Spool custody, gaps and bounds. |
| Delivery | [Delivery](delivery.md) | ACK rule, delivery kernel and layer split. |
| Control | [Central control](control-plane.md) | Enrollment, desired/applied configuration, pause, resume and revoke. |
| Identity | [Identity and access](identity-access.md) | Local owner passkeys, reader tokens and Spindle tokens (three fixed roles), sessions and durable policy implemented; finite verification is recorded in the matrix. |
| Hardening | [Security hardening](security-hardening.md) | Peer admission, poisoned-lock refusal, OTLP deadlines, secure log opening, credential-file policy and the unit sandbox, as enforced now. |
| Console | [Operator PWA](../milestones/operator-console.md), [algorithm model](console-algorithms.md), [art direction](console-art-direction.md) | Authenticated Leptos/WASM workflows and bounded models implemented; complete browser/device acceptance remains required. |
| Storage | [Storage](storage.md) | `FAB1` frame log, Spool and server journal; the archived FOL2 log format. |
| History and query | [Retained history and query](retained-history.md) | Segments, retention, exact queries, completeness, freshness and pages. |
| Sealing | [Sealer](sealer.md) | Bounded external merge writer adopted; finite builder/recovery and companion soak evidence is linked from [CURRENT](../CURRENT.md). No whole-server memory proof ([ADR-0022](../decisions/ADR-0022-build-segments-by-external-merge-sort.md)). |
| Observation codec | [Observation record and FOB1 encoding](observation.md) | Accepted and implemented for in-memory blocks in the opt-in Walk tail ([ADR-0023](../decisions/ADR-0023-define-an-observation-record-with-a-canonical-encoding.md), [ADR-0024](../decisions/ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md)). Wire, journal and Segment adoption are separate decisions. |
| Deployment | [Linux deployment](deployment.md) | Service identity and systemd boundary; fresh Debian VM installation acceptance passed its registered checks. Exact release packages and Fedora remain separate gates. |
| Machine policy | [layers.json](layers.json), [core-purity.json](core-purity.json) | Enforced by `cargo xtask`. |

## Legacy demonstration

| Views | Scope |
| --- | --- |
| [Local input and batching](ingestion.md) | Original single-process pipeline; its code is in the [research archive](https://github.com/kmosoti/FabricO11y-research). |

## Research views

| View | Scope |
| --- | --- |
| [Query research](query.md) | Coverage receipts and exact predicates. |
| [Local research prototype](research-prototype.md) | Offline adapter, immutable snapshot, external root and checkpoint resume. |
| [Agent telemetry](agent-telemetry.md) | Development-workflow observation. |

See the [documentation landing page](../README.md) for reading order and source-of-truth hierarchy.
