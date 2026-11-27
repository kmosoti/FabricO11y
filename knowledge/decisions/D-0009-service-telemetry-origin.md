# Separate Spindle origin for service telemetry

Sharing a workload origin would couple its contiguous forwarding prefix to
service diagnostics. A dedicated Spindle instance separates local quotas and
forwarding cursors while retaining the SDK's existing fixed buffers, admission
outcomes, byte-identical retry, and custody receipt checks. Per-origin canonical
capacity requires receiver accounting. The local queue reports its outcomes
without producing a new observation for each retry.

An independent destination could remain available during a canonical receiver
outage. Using the configured authority keeps operation consistent with the rest
of the pipeline and lets the telemetry Spindle retain accepted data through a
bounded outage. Local counters and custody state remain available to diagnose
blocked forwarding.
