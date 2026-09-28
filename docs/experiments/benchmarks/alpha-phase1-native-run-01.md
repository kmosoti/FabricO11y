# Phase-1 native node: local qualification run 01

Status: these three local native trials passed on 2026-09-27 on the exact binaries hashed below. Cross-family review then found four phase-1 defects. Focused repairs postdate these measurements, so this run is retained as pre-repair evidence and final phase-1 acceptance is pending re-review. It is not an ACK, freshness, server, fleet or installation result.

## Registered method and environment

The [protocol](alpha-phase1-native-protocol.md) fixed three seeds, 15 s warmup plus 120 s measured, 2 offered newline logs/s, exactly 512 body bytes alternating repetitive and seeded high-entropy text, and a 15 s metric interval. Trials ran sequentially through the owned [bounded runner](../../../tools/qualification/runner.py) with 180 s, 300 MiB and 1 MiB per-invocation duration/live-data/evidence limits. Its polling limit is not a filesystem quota. The native writer checks its 1 MiB source cap before each append. All three runner invocations exited `0`, reported `passed=true`, no stop reason, no leftover descendant and an inner command exit `0`. No trial hit a budget.

Environment: Debian GNU/Linux 13.5, WSL2 Linux `6.18.33.2-microsoft-standard-WSL2`, x86_64, target filesystem reported as `ext2/ext3`, 12 visible logical CPUs, Rust/Cargo 1.98.0, Python 3.13.5. No four-CPU server restriction or deployed systemd resource limit was exercised. Build command `CARGO_HOME=$PWD/target/alpha-cargo cargo build --offline --locked --release --bins --example alpha_native_dump` exited `0`. The measured binaries' SHA-256 values were:

| Artifact | SHA-256 |
| --- | --- |
| `target/release/fabric-node` | `193b1676e4d403216476487e870e2f8f5b71af200c5f8180250e4c9e2313ef7a` |
| `target/release/fabricctl` | `9da07efb06991e8bbbf0075ca6b3efd3b48af88455a2f098fe82ffb711986658` |
| `target/release/examples/alpha_native_dump` | `f660cf3a73fee5ec5d37e6c9c1a8c5058270e2eedbfca2e570bc74146b13c883` |
| `tools/alpha/native_phase1.py` | `b503193fd3b8b930bcf33223af6032d3ce47fce1e4fe6b22f396b705826463e8` |

For each `N=1,2,3`, the exact command was:

```sh
python3 -B tools/alpha/runner.py --out "target/alpha-native-qualified-seed${N}" --duration-s 180 --disk-bytes 314572800 --max-output-bytes 1048576 -- python3 -B "$PWD/tools/alpha/native_phase1.py" --seed "0xA11FA00${N}" --node-bin "$PWD/target/release/fabric-node" --ctl-bin "$PWD/target/release/fabricctl" --dump-bin "$PWD/target/release/examples/alpha_native_dump"
```

The compact [raw summaries and runner results](data/alpha-phase1/) preserve invocation IDs, source hashes, command arguments, exits and sampled resource peaks. Earlier preliminary trials used a prior binary and, for seed 1, a script without inline exact replay; they are not this qualifying result and remain ignored under `target/alpha-implementation/native-seed*.log`. The historical phase-0 FOL2 run `06` was not rerun.

## Results

Each source offered 270 lines (30 warmup, 240 measured) and 138,510 source bytes. The node committed 270 log records with zero gaps, and the independent dump of persisted OTLP log bodies matched the source SHA-256 exactly, including after node restart. This host emitted 75 metric points per 15 s sample, 750 across ten samples in warmup and measurement; this differs from the fleet fixture's fixed 32 points. A post-restart `collect` added one more metric batch and no duplicate log record.

| Seed | Source and replay SHA-256 | Offered/committed logs | Native CPU ticks converted to s | Node VmHWM KiB | Restart ms | Runner sampled peak live bytes |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| `0xA11FA001` | `fc552e040f7383381e2a45b6be7ff039d197d8a4b740e7bf1406f9ebcc6a6cfa` | 270/270 | 0.01 | 2716 | 19.458 | 418096 |
| `0xA11FA002` | `4cf543085ebf11e3807c2b3fc40ceee206560054300013938fd6f62260b6e056` | 270/270 | 0.00 | 2560 | 21.442 | 418100 |
| `0xA11FA003` | `88e24ee87a58a2e4c13651efd9ec594b6fd404ecfdae84ac1860b8ce96d08244` | 270/270 | 0.00 | 2748 | 17.503 | 418100 |

The CPU counter was sampled at elapsed 15.000 s and 135.115–135.119 s, so its window includes up to 0.119 s of drain after the nominal 120 s measurement. Linux clock ticks resolve only 0.01 s here; 0.00 means fewer than one observed tick, not zero CPU. VmHWM is the native node's kernel high-water RSS; the runner's separate 24 MiB approximate tagged-descendant RSS includes the Python generator and is not the node gate. The highest native VmHWM was 2748 KiB, below the registered 64 MiB gate.

Each trial had 268,865 OTLP payload bytes and 270,452 committed journal bytes before restart. From the **same post-restart snapshot**, 275,287 OTLP payload bytes and 277,057 live spool bytes, including the 28-byte identity, give spool amplification `1.006430`. End-of-trial all-live owned data before the small summary was 416,432 bytes, including source log, spool, config and node output. The runner's separately sampled peak, up to 418,100 bytes, includes transient and evidence files. Per-run retained evidence was 1,639–1,641 bytes; all three disposable roots occupied about 1.4 MiB before safe cleanup. Maximum source schedule lag was 0.700 ms. Log collection currently follows the 15 s metric loop, so this run does **not** establish the later ≤5 s observation-to-query freshness gate.

## Correctness controls and limits

`CARGO_HOME=$PWD/target/alpha-cargo cargo test --offline --locked --all-features` exited `0`: 3 library tests, 2 alpha journal, 4 alpha log-reader, 6 alpha node and the unchanged 25 legacy integration tests passed. The phase-1 tests exercise CPU/disk counter starts and resets across restart and boot change, denied host/log reads and recovery, rotation, truncation, incomplete lines, oversized skip across restart, spool full with unknown coverage, framing corruption, plausible interrupted tail, and injected reported failures after data sync, marker sync and pre-clear directory sync. The journal writer quarantines on each fault, leaves the recovery witness and refuses reopen. The copied independent journal oracle exited `0` with six API cases, ten selected corruptions and 72 length-matrix mutations; [framing evidence](../formal/alpha-journal-length-repair.md) records the original failing counterexample and the mechanical API adapter. The independent log probe's valid, oversized and FIFO cases each exited `0`; [reader evidence](../formal/alpha-log-reader-repair.md) preserves the original failures. `python3 -B tools/alpha/test_runner.py` exited `0` (18 tests) and `python3 -B tools/alpha/test_workload.py` exited `0` (5 tests), including phase-0 budget and source negative controls.

The tested sync seam injects a reported error after successful syscall return; it checks quarantine and retained recovery authority, not actual failing media. Successful `sync_all`/filesystem semantics and an independently retained source are required. No sudden power-loss test was run. The local line reader cannot detect a truncate-and-regrow past its prior offset between samples. Read-only inspection hides counts while a recovery marker exists; it is not a network durability receipt. Fleet throughput, ACKs, query latency, outage drain and systemd packaging remain later-phase gates.
