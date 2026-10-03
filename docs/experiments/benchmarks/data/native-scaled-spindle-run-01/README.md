# Native scaled Spindle evidence

See [results](../../native-scaled-spindle-run-01.md), [original protocol](../../native-scaled-spindle-protocol.md) and [retry protocol](../../native-scaled-spindle-protocol-r2.md). Completed medium used harness `d837707`; completed enterprise retry used `9eb15a7`. Raw owned directories: `/workspace/scratch/native-scaled-run-01/medium`, `/workspace/scratch/native-scaled-run-02/enterprise`. Original application files/native state/SQLite audits remain there. Summary completion was approximately 63.47 / 197.59 seconds after trial directory creation, including replay and audit. Interrupted first enterprise remains separately recorded; do not pool its events with the retry.

Each completed tier retains:

- `environment.json`: source/harness revision, binary SHA-256, affinity, kernel and cgroup limits.
- `summary.json`: registered gates, precision counts, latency and process/inventory outcomes.
- `resources.json`: once-per-second RSS/CPU, native progress, logical storage, proc IO and shared cgroup memory.
- `source-clocks.csv`: node, tick, offered count, scheduled realtime ns, post-write realtime ns, completion lateness ns.
- `source-file-hashes.json`: SHA-256 of each original app file. The deterministic recipe is [native_source.rs](../../../../../tools/bench/native_source.rs).
- `clock-groups.jsonl.gz`: `[node,tick,collected_ns,received_ns,ack_stdout_ns,source_postwrite_ns,scheduled_ns,phase,record_count]`.
- `batch-hashes.jsonl.gz`: `[node,sequence,exact_recovered_Batch_SHA256,encoded_bytes,server_receive_ns]`.
- `nodeXX-events.jsonl.gz`: native collection-cycle and delivery-attempt lines, timestamped at stdout observation.
- `dimensions.json`: post-hoc CPU windows, independently recomputed counted-clock percentiles/ACK map check and additional inventories; not new acceptance thresholds.
- `storage-and-generation.json`: final source/Segment logical sizes and actual phase write completion boundaries.
- Native stderr/producer stdout and controls; credentials/private keys excluded.

Compressed/compact evidence is about 0.88 / 1.75 MiB per tier. Retained clock/count/hash evidence supports reanalysis, but a fresh body audit requires original scratch state or a newly reproduced trial. The bundle carries repository evidence, not multi-GiB raw scratch state.

Verification:

```sh
python3 -B tools/bench/analyze_native_scaled.py docs/experiments/benchmarks/data/native-scaled-spindle-run-01/medium
python3 -B tools/bench/analyze_native_scaled.py docs/experiments/benchmarks/data/native-scaled-spindle-run-01/enterprise
python3 -B tools/bench/check_native_scaled_controls.py docs/experiments/benchmarks/data/native-scaled-spindle-run-01
cargo xtask checks --profile fast
bun tools/docs/check.mjs
```

Both analyzer invocations and rejecting controls exited 0. [Controls](post-hoc-negative-controls.json) accept exact source bytes and reject changed/missing SQLite entries, reject a +1 ms reported percentile mutation, reject a corrupted recovered Batch hash, and reproduce the disappearing-Spool-file monitor race without terminating the sampler.

Fast checks on runtime/harness revision `9eb15a7`: **exit 1, 19 passed / 1 failed**; [output](fast-checks.txt) and [receipts](receipts/clippy.json). Clippy's underlying exit was 101: existing Rust 1.98 `chunks_exact_to_as_chunks` diagnostic in `crates/fabric-observation/src/crc32.rs:81`. No gate or production code was weakened. Full workspace tests, properties, simulations, independent oracles, documentation and structural checks passed in that run. Documentation was also checked after adding this report; final output is [docs-check.txt](docs-check.txt). `git diff --check` exited 0.

SHA-256 artifact inventory is in `manifest.sha256`; it excludes itself and this README. No remote publication, main merge or deployment is claimed.
