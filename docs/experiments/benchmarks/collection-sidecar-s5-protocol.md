# S5: bounded collection adapter and conservative sidecar simulation

Status: registered before implementation/timing. The [adapter profile](../../../tools/storage-probe/COLLECT_API.md)
and [lifecycle CLI](../../../tools/storage-probe/CLI_API.md) define accepted inputs,
observable rejections, tenant/source assignment, retry and ownership. This increment
uses an offline Log export file adapter; a conformant network receiver remains future
work. [OTLP JSON encoding](https://opentelemetry.io/docs/specs/otlp/#json-protobuf-encoding)
defines decimal-string 64-bit fields. The narrower accepted profile and event mapping
are explicit Fabric experiment choices.

## Correctness

Freeze independent mapping fixtures before implementation. Include ordered resource,
scope and log attributes, duplicate keys, all supported scalar types, omitted versus
zero timestamps, signed zero, Unicode bodies, ID/time overflow, malformed JSON,
unsupported values, invalid hex IDs, nonzero dropped counts and multi-resource input.
Reject the whole batch before opening an output or journal; no invisible partial
acceptance. Different caller tenant configs must remain distinct. Then use actual
separate CLI processes to adapt, ingest through a capacity-1 buffer, replay and compare
all mapped Events. Identical retry adds no log bytes. Inject a checker defect that
drops one log record and one that trusts a telemetry tenant attribute as authority.

## Sidecar hypothesis and fixed workload

A source-built conservative summary may reduce central builder work, but validation
and transport may erase the gain. Use S1 mixed and shuffled-Log events, seeds
201/202/203, 2,048 events, 64-row blocks, identical retained source bytes. One warmup
and five alternating measured trials per shape/seed. No raw events are filtered.

Baseline: server reads retained source and builds SealedSnapshot summaries/root.
Sidecar: separate source process reads those events, builds per-block E1 summaries
and serializes JSON hints; server reads the same source plus hints, validates every
summary against raw rows and constructs the same immutable root. Record source CPU
and wall time separately, server CPU/wall, hint bytes, source bytes, whole-process
RSS and total CPU. Phase boundaries include file reads and JSON parsing; source
fixture generation stays outside. Transfer bytes are logical file payload sizes,
not measured network traffic. Readings on this shared host need load metadata.

Every variant must produce the same anchor and independent full-query results.
Missing/corrupt/stale/underinclusive hints must fall back to central rebuilding,
never remove rows or suppress an error in the source itself. Retain a counterexample
for a sidecar hint that omits a known token. Sidecar source and server execute
sequentially to avoid conflating overlap with total CPU cost.

## Decision and limits

Pre-registered benefit requires median paired server CPU <=90% of baseline and
median total source+server CPU <=110%, with all equality/rejection gates passing.
Report both conditions even if one fails; report hint-byte overhead and all trials.
Negative results preserve the central builder as default. This tests source-side
summary construction, not eBPF, sampling, DPDK, Homa, GPU or DPU execution. No raw
filtering is justified by an uncertain summary. Real network OTLP and hardware
experiments remain conditional on facilities and equivalent correctness semantics.

## Timing implementation boundary (registered before the harness)

On this Linux host, phase CPU uses CLOCK_PROCESS_CPUTIME_ID alongside Instant wall
time. Record timer-call overhead with an empty calibration loop; do not subtract a
noisy estimate from samples. Whole-process CPU/RSS are reported separately and include
setup/checks. Gates using server/build/query CPU refer to the named measured phase,
not the sum of unrelated validation work. All results must retain these boundaries.
