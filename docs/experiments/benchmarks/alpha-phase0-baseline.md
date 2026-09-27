# Alpha phase 0: current journal baseline

## Registered question and boundary

Before implementing alpha delivery, measure the existing FOL2 `EventLog` with fresh commands. This is a **legacy Gauge-only baseline**, not an alpha log/metric workload and not a comparison against the unfinished FAB1 draft. The existing [Stage 6 result](local-log-stage6.md) is historical and will not be copied into this result. Successful FOL2 append still requires its event-data sync and commit-marker sync. An alpha comparison may use these numbers only if it explicitly preserves the workload, build, host and durability semantics.

## Protocol fixed before execution

Build `local_log_probe` in release mode with `cargo build --release --offline --locked --example local_log_probe`. Use one unmeasured warmup and three fresh measured files with decimal seeds `2703204353`, `2703204354`, `2703204355` (hex `0xA11FA001` through `003`), 200 generated Gauge events per file. Run each write and verify in separate processes under `tools/alpha/runner.py` with a 60 s command cap, 16 MiB trial disk cap, and 2 MiB evidence cap. The independent verify process reconstructs the generator and compares exact encoded event contents and count. A failed write, verify, time or byte cap invalidates that trial. The current source generator is deterministic but its workload does not include the proposed 2 logs/s or 32 metric points/15 s.

For each trial report write pipeline events/s (`200 / ingest elapsed_ns`), append p50/p99 nearest rank, file bytes/event, reopen and replay milliseconds, whole-process VmHWM if emitted, and the exit and limit result. Preserve trial CSV and `sha256sum` locally under ignored `target/alpha-phase0-baseline-*`, with a compact result and source hashes here. No latency target is asserted from this baseline. This is process restart evidence under successful sync assumptions, not physical power-loss proof.

## Execution

Executed on 2026-09-27 UTC in WSL2 kernel `6.18.33.2-microsoft-standard-WSL2`, Intel i7-10750H, x86_64, repository on `/dev/sdd` ext4 (`data=ordered`). Rust `1.98.0`; release build with default flags. The checked-out HEAD was `43d709b7a3acafd3c2ccbe3a44cd74df64e2d55c`, with uncommitted alpha work. Hashes of every relevant source and lock file are in the committed [compact summary](data/alpha-phase0/summary.json) and its ignored original at `target/alpha-phase0-baseline-06-record/summary.json`. The summary SHA-256 is `302577fb1a55c972b2cc83879975ea756dabb82aac6263fcef5428e75569a771`. The FOL2 probe source SHA-256 was `6874e45fd769544add0b3be89af0b868821ef47fe6fc70c8f53313219e22ff99`; the runner source was `a3b31fc31675462f5962a01a4ee66650cc45c483fb123b51092c98984ac51452`. Local attempts `01` through `05` used earlier harness revisions or the previous release build; their ignored artifacts remain for audit. Only run `06` is reported below.

```sh
CARGO_HOME="$PWD/target/alpha-cargo" cargo build --release --offline --locked --example local_log_probe
python3 -B tools/alpha/run_baseline.py --prefix alpha-phase0-baseline-06
```

Both commands exited `0`. The wrapper ran a warmup plus three fresh trials. Every trial's write and verify child exited `0`, runner `passed=true`, with no stop reason. The verify child independently reconstructed and compared all 200 events in each file. Raw CSV, log, runner manifests and SHA-256 values remain under ignored `target/alpha-phase0-baseline-06-{warmup,trial-1,trial-2,trial-3,record}`. Each trial wrote a 29,600-byte FOL2 file (148 bytes/event); no full-buffer rejections occurred.

| Trial / seed | Pipeline events/s | Append p50 / p99 µs | Open / replay ms | Probe VmHWM KiB |
| --- | ---: | ---: | ---: | ---: |
| 1 / `2703204353` | 253.01 | 3341.7 / 10904.6 | 3.274 / 0.534 | 2184 |
| 2 / `2703204354` | 255.76 | 3353.5 / 11500.7 | 6.995 / 1.004 | 2172 |
| 3 / `2703204355` | 255.53 | 3331.8 / 10512.2 | 3.575 / 0.524 | 2340 |

The result is conditional on successful syncs and this synthetic Gauge shape. It does not establish a server ACK latency, retention cost, node RSS, multi-node throughput, or any alpha performance gate. WSL host activity and the short 200-event sample limit comparison; no competing design was measured here.

The [phase-0 review repairs](alpha-phase0-review-response.md) changed the runner **after** run `06`. Its committed summary preserves the exact original runner SHA-256 above; this historical measurement was not rerun or relabeled as a result of the repaired runner.
