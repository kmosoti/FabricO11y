# D-0218: Bounded structured metric labels

Use a dedicated opt-in metric descriptor so existing wire shapes and emitted
defaults keep their meaning. Keep caller-supplied dimensions under `labels` and
classify the entire decoded key map before accepting bytes into custody.

Use a length-delimited logical key for producer, resource, metric name, unit,
type and sorted labels. Sequence, timestamp, value, reset, generation and run
context describe individual observations. Local Spindle bounds logical series
and lease memberships within record admission. The canonical receiver needs its
own bound because authenticated origins can submit directly. It scopes that
bound by origin and includes changes in the raw record transaction before
engine publication.

Retire local accounting with safely pruned generations and canonical
accounting with a one-hour server-owned inactivity window. A receiver clock
step forward can release entries early in elapsed real time. A backward step
is clamped to the persisted accepted floor. Neither retirement changes the
accepted raw bytes. Preserve per-record sequence and provenance through
Gorilla and Parquet projection.
