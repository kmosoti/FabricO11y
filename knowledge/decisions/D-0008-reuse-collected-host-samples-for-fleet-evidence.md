---
id: D-0008
created: 2026-11-18
added_by: agent
human_ratified: false
---

# Reuse collected host samples for fleet evidence

A local host collector already reads pressure, memory, cgroups, systemd unit
states, and filesystem churn. The opt-in fleet export transforms a fixed
subset of that snapshot into normal Spindle metric observations. The
receiver authenticates the persistent Spindle origin, and the metric
resource records a configured node name. The authenticated origin identifies
the source across reused node names, and local collection continues during
disconnection.

The exporter uses a small volatile queue before durable Spindle admission.
Backpressure is accounted locally. Lost acknowledgments retry the original
bytes and identity. The same local data is already available through the
collector, so its projection advances past the marked export copies
without publishing them as a second set of rows.

Revisit the fixed signal set if an operational scenario requires a signal
that the local snapshot cannot already provide. Filesystem paths and
individual process and unit names remain local.
