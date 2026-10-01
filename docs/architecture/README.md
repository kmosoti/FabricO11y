# Architecture views

Product architecture (current behavior; status words follow the [evidence states](../QUALIFICATION.md#evidence-states)):

- [System](system.md): runtime components, crate layers and the dependency rule.
- [Spindle](spindle.md): host collection, Spool custody, gaps and bounds.
- [Delivery](delivery.md): the ACK rule, the delivery kernel and its layer split.
- [Central control](control-plane.md): enrollment, desired and applied configuration, pause, resume and revoke.
- [Storage](storage.md): `FAB1` frame log, Spool and server journal, and the FOL2 log format.
- [Retained history and query](retained-history.md): Segments, retention, exact queries, completeness, freshness and pages.
- [Linux deployment](deployment.md): service identity and systemd boundary; installation not yet run.
- [Sealer](sealer.md): how a sealed journal file becomes a Segment. **Accepted design, not yet implemented** ([ADR-0022](../decisions/ADR-0022-build-segments-by-external-merge-sort.md)); the page also says what the sealer does today.
- Machine-readable policy: [layers.json](layers.json) and [core-purity.json](core-purity.json), enforced by `cargo xtask`.

Legacy demonstration:

- [FOL2 demonstration](fol2-demo.md) and [local input and batching](ingestion.md): the original single-process pipeline.

Research, outside the product:

- [Query research](query.md): coverage receipts and exact predicates.
- [Local research prototype](research-prototype.md): offline adapter, immutable snapshot, external root and checkpoint resume.
- [Agent telemetry](agent-telemetry.md): development-workflow observation.

The [documentation landing page](../README.md) gives the reading order and the source-of-truth hierarchy.
