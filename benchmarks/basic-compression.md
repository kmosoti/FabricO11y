# Basic compression correctness check

The MVP gate includes a deliberately small compression smoke check, not a throughput claim. It
creates 128 structured observations, seals them through the production segment encoder, decodes
the result, checks record parity, and requires the compressed payload to be smaller than canonical
JSONL for this repetitive fixture.

Run it with:

```text
cargo run -p fabric-segment --example compression-smoke --quiet
```

The command emits JSON containing record count, uncompressed bytes, compressed bytes, and ratio.
Later performance work may add stable hardware and corpus methodology without weakening this
correctness oracle.
