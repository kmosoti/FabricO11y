# Typed batch representation evaluation

Arrow is an experimental batch format in `xtask`, not a production SDK dependency
or a replacement for the SDK-to-Spindle JSON protocol. Local SDK collection and
SQLite custody do not require Arrow. See [collection](collection.md).

`representation-evaluator arrow-run development` compares direct JSON, direct
binary rows, and Apache Arrow IPC with equivalent bounded batches and native batch
custody. `arrow-run held-out` uses a separate fixed seed. Each configuration has
one warmup and five measured samples in fresh processes. Build with
`--features allocation-metrics` for a separate allocation campaign; do not use its
timing numbers. The existing representation report consumes the JSONL outputs.

The direct encoders stream validation without allocating a full-batch JSON value
tree. Arrow uses seven UTF-8 columns, six unsigned integer columns and one Float64
column. A variant code determines interpretation of payload columns. Only dynamic
JsonLog fields retain JSON encoding. This initial flat schema uses neither dictionary
encoding nor compression; results do not rank every possible Arrow layout. Binary
rows use the same typed logical fields. JSON preserves the v2 names and envelope.

The Arrow reader accepts exactly the emitted fixed schema, one record batch and
an end marker. Before the generic reader it checks metadata/body bounds, row and
node counts, exact buffer sizes and alignment; it rejects compression, dictionaries
and variadic layouts. Materialization checks expanded string limits before cloning.
All codecs enforce at most 32 records, 256 KiB canonical JSON and a 1 MiB frame.
This bounds codec inputs and structures, not arbitrary process RSS. The generic
Arrow buffers, output strings and dynamic JsonLog values may coexist in memory.

Both seal latencies refer to the experimental local three-process pipeline. The
sender is a codec harness, not the production SDK; the latter retains its fixed
payload reservation. Measurements report Rust allocations separately from process
RSS and native SQLite allocations. Projection measurements materialize observations
before projection; they do not measure Arrow-native vectorized query performance.
No benchmark establishes globally minimal memory, remote deployment durability or
an automatically preferred production transport.
