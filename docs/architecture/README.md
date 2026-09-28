# Architecture views

- [Current application system](system.md): Rust demo and FOL2 boundary.
- [Local input and batching](ingestion.md): event ownership and full-buffer retry.
- [Local storage](storage.md): FOL2 commit and recovery.
- [Query research](query.md): coverage and exact predicates.
- [Local research prototype](research-prototype.md): offline adapter, immutable snapshot, external root and checkpoint resume.
- [Delivery ownership](delivery.md): modeled ACK rule and local commit.
- [Planned Linux alpha deployment](deployment.md): accepted service identity and systemd boundary, not yet installed.
- [Central control](control-plane.md): enrollment, desired and applied configuration, pause and revoke.
- [Native Linux node](node.md): phase-1 local collection and spool contract, in progress.

The [documentation landing page](../README.md) separates application behavior from research tooling. The [current state](../CURRENT.md) records checks and open gates.
