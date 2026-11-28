# D-0223: Decouple opt-in live cursor polling from host ticks

The local live metric cursor is optional and shares bounded admission and
watermark handling with the collector's Spindle projection. When it is enabled,
a Spindle socket is configured, and the normal collection interval exceeds one
second, a one-second skipped-tick poll runs on the same serialized projection
path. It does not advance the host tick count or cause an idle flush.

Backpressure pauses the additional polls until a normal tick services the
existing flush and retention paths; failed flushes keep that pause. Normal
collection, self-shedding and durable watermark rules remain authoritative.
This is a best-effort local cadence, not a latency or custody guarantee.
