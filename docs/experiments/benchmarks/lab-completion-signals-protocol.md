# Native signal and lifecycle collection addendum

Registered before these workloads. Implements R2a, the ordinary-load portion of
C4, and fresh C3 lifecycle in the [completion campaign](lab-completion-protocol.md).
The new private `signals.py` leaves historical native runners and independent
oracles unchanged. These are local observations, not installation qualification.

First run a 30-second trace integration preflight, then 180-second trace-only,
development logs/metrics and mixed, and small logs/metrics and mixed cells.
Development has one Spindle at 10/30/10 logs/s; small has twenty Spindles at
1,000/3,000/1,000 aggregate logs/s over 60/60/60-second windows. Bodies retain the
900-byte alternating repetition/seeded-entropy distribution, seed 2703163393.
TLS, 15-second host metrics, 64 MiB natural journals and one seal worker apply.
The server uses Scan, two CPUs; nodes, SDK and coordinator use the next two CPUs.
The trace-only fixture still allows host metrics; it offers no file logs.

Python OpenTelemetry SDK and OTLP/HTTP exporter are pinned to 1.38.0 in an owned
data-drive venv. Preserve its complete resolved dependency versions and inspect
the installed exporter implementation before using private observation hooks.
One SDK targets node00's loopback receiver at ten parent/child pairs/s for
development and fifty pairs/s for small. Each pair has explicit nanosecond
timestamps, recorded SDK-generated IDs, parent relationship, fixed names,
three attributes, one event, and ERROR status every tenth pair. Batching uses
queue4096, batch64, delay1000ms and timeout5000ms. Preserve exact request bytes,
each HTTP attempt, local response and flush/shutdown result. Actual encoded
bytes are observed, not assumed equal to log bytes.

During each short cell gracefully restart the server halfway through, with a
two-second stopped interval while nodes retain custody. Keep source lateness and
query failures during that deliberate interruption visible. Query every ten
seconds; after drain, compare complete logs, CPU metrics and spans pagination
chains to the unchanged Python oracle. The log chain selects node00 and prefix
`load-00:000`; source/hash comparison still covers all emitted logs on every
node. Preserve source ledgers, complete query pages, every Batch custody digest,
and exact trace/metric protobuf; remove duplicated raw source-log payloads only
after grading. This compact evidence is not a full byte-replay archive.
Separately require exact source log
hashes, source span IDs/parents/names/timestamps, exact concatenated successful
SDK payload bytes versus retained raw traces, every server ACK hash recoverable,
no duplicate or missing Batch, and successful SDK flush. Metrics are compared
to independently decoded retained payloads, not fixed host-value predictions.
These integration cells establish correctness through a server restart; they do
not substitute for node-restart or overload evidence. Query/local-response timing
is descriptive; deliberate restart intervals are not steady latency acceptance.

After successful mixed integration, repeat development and small mixed cells
with verified nested application cgroups. Development server/node maximums are
512/128 MiB, high384/96 MiB. Small maximums are3072/256 MiB, high2560/192 MiB.
Server plus node00 model one host under aggregate3328 MiB/high2816 MiB/tasks640
(development640/480 MiB); other nodes model separate edges, each with its own
256 MiB cap. Server/node task limits512/128; CPU quotas2/1 cores. The aggregate
20 GiB outer cap and zero swap remain. The ordinary-limit decision requires no
application OOM plus all integration correctness gates. Pressure/gap and node
restart experiments remain separate; do not call these cells a complete C4 pass.

Fresh lifecycle: empty development state, logs/metrics only at constant10logs/s,
same payloads, no artificial restart or seeded history. Freeze12000seconds of
offered load before admission, followed by20seconds drain. At observed1188–1196
encoded bytes/log, two64MiB thresholds predict roughly11223–11296seconds; the
prediction must be checked against actual Segment publication and journal
reclamation. Require at least two natural rotations and exact final custody/query
answers. Record real/monotonic/boottime clocks, sampled journals/Segments,
whole-job process and cgroup resources. Interrupted clocks make timing inconclusive.
This run does not establish default retention eviction. All scratch is owned;
preserve compact failure evidence before cleanup. Global campaign caps apply.

Negative controls must reject lost, altered and duplicated source identities.
The frozen query oracle retains its existing independent controls. Full source,
configuration and binary hashes accompany every trial; never silently alter
historical decision rules after observing outcomes.
