# Phase-1 native node: post-repair run 02

Status: three registered trials passed on 2026-09-27 on the final phase-1 binaries (plan step P1.4). This replaces [run 01](alpha-phase1-native-run-01.md) as the phase-1 native evidence, because run 01 predates the review repairs, the Unicode gap fix, the crash-recovery change and the lock fix. It is not an ACK, freshness, server, fleet or installation result.

## Method and environment

Unchanged from the [registered protocol](alpha-phase1-native-protocol.md) and run 01: three seeds, 15 s warmup plus 120 s measured, 2 offered newline logs/s of exactly 512 body bytes, a 15 s metric interval, sequential trials under the frozen [runner](../../../tools/qualification/runner.py) with 180 s, 300 MiB and 1 MiB limits. The runner and [native harness](../../../tools/qualification/native_phase1.py) SHA-256 values equal the frozen phase-0 and run-01 values, so the harness did not change. Environment: Debian GNU/Linux 13, WSL2 kernel `6.18.33.2-microsoft-standard-WSL2`, x86_64, 12 logical CPUs, target filesystem reported as `ext2/ext3`, Rust 1.98.0. The binaries were built from commit `c51d3a8a38151f0fe2a067415f312a18ca545622` with `cargo build --offline --locked --release --bins --example alpha_native_dump`, exit `0`.

| Artifact | SHA-256 |
| --- | --- |
| `target/release/fabric-node` | `32ae80bc49cdba2cdf3f70dfd6b06ec06b09e5c68347677af9e534cc67a64936` |
| `target/release/fabricctl` | `1f6a60a3a3377fd26ce3899911579060704b78a1d098f41cabfad911d7a245a0` |
| `target/release/examples/alpha_native_dump` | `6c93ea5301635267bd14dfa31c80288d89ff88f759bf2defd89ba02b281e00e9` |
| `tools/alpha/native_phase1.py` | `b503193fd3b8b930bcf33223af6032d3ce47fce1e4fe6b22f396b705826463e8` |
| `tools/alpha/runner.py` | `2425e95028edd13a4a43e49fc99904ce695b0b98aafe6d277124b1b169b5c49d` |

For each `N=1,2,3` the command was:

```sh
python3 -B tools/alpha/runner.py --out "target/alpha-native-run02-seed${N}" --duration-s 180 --disk-bytes 314572800 --max-output-bytes 1048576 -- python3 -B "$PWD/tools/alpha/native_phase1.py" --seed "0xA11FA00${N}" --node-bin "$PWD/target/release/fabric-node" --ctl-bin "$PWD/target/release/fabricctl" --dump-bin "$PWD/target/release/examples/alpha_native_dump"
```

All three runner invocations exited `0` with `passed=true` and no stop reason. The compact [summaries, runner results and hashes](data/alpha-phase1/run02/) are tracked.

## Results

Each trial offered 270 lines and 138,510 source bytes, and committed 270 log records. The persisted log bodies' SHA-256 equals the source SHA-256 for every seed, and equals the run-01 source hash for the same seed. The host emitted 750 metric points across ten 15 s samples.

| Seed | Offered/committed logs | Native CPU s (measured window) | Node VmHWM KiB | Restart ms | Spool amplification | Runner peak live bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `0xA11FA001` | 270/270 | 0.04 | 2624 | 40.0 | 1.006615 | 417270 |
| `0xA11FA002` | 270/270 | 0.04 | 2600 | 21.5 | 1.006615 | 417270 |
| `0xA11FA003` | 270/270 | 0.04 | 2736 | 28.0 | 1.006615 | 417266 |

The highest VmHWM is 2736 KiB, below the registered 64 MiB gate. Measured CPU rose from at most 0.01 s in run 01 to 0.04 s. The likely cause is the new `run` loop, which wakes every 100 ms to honour a stop request; this was not isolated. Clock ticks resolve 0.01 s here. Maximum source schedule lag was 9.3 ms.

## Limits

This is one native process on one host. It does not measure network delivery, ACK latency, observation-to-query freshness or fleet tiers. The crash-recovery behavior is covered separately by the [kill probe](../../../tools/qualification/kill_probe.py) and unit tests recorded in [ADR-0011](../../decisions/ADR-0011-separate-interrupted-append-from-known-failure.md).
