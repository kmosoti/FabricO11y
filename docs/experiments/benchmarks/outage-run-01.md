# Outage buffering and drain, run 01

Status: the three trials of the registered [outage protocol](alpha-phase5-outage-protocol.md) ran unchanged on 2026-09-28. All three passed every gate. The protocol pins no CPUs, so it ran as written on a four-CPU host. The host is not the target profile, so this is **Measured and passing**, not a target-profile qualification.

This run replaces the earlier interrupted attempt, which produced no result, as the outage evidence.

## Method

As registered, [`outage_drain.py`](../../../tools/qualification/outage_drain.py) runs one `fabric-server` and one enrolled `fabric-node run`. A writer offers two 512-byte lines per second throughout the trial. The server:

1. runs for 60 s;
2. is stopped with SIGTERM for 1,800 s;
3. is restarted.

Drain time runs from the restart until the node's acknowledged sequence reaches the last sequence it had committed at the restart.

**Artifacts.** The binaries, the examples and the harness were built with `cargo build --release --locked --workspace --bins --examples`. Their Rust sources are unchanged since `63bbeac`: the binaries are byte-identical to the frozen history set. They were frozen into `target/alpha-p5-frozen`, with [26 SHA-256 values](data/delivery-recovery/outage/hashes.txt); the recorded source commit is `bb8d06d`. `sha256sum -c` after the run found 0 mismatches.

**Host.** 4 logical CPUs, 15 GiB RAM, Ubuntu 24.04 in a Firecracker VM, ext4, Python 3.11.15. For each seed `N`:

```sh
python3 -B tools/qualification/runner.py --out target/alpha-p5-outage-seedN --duration-s 3000 \
  --disk-bytes 1073741824 --max-output-bytes 1048576 -- \
  python3 -B $PWD/target/alpha-p5-frozen/tools/outage_drain.py --seed 0xA11FA00N --bin-dir $PWD/target/alpha-p5-frozen
```

All three runner invocations exited 0 with `passed=true`, no stop reason and cleanup confirmed. Timings are in [progress.txt](data/delivery-recovery/outage/progress.txt), and the summaries and runner results are in [data/delivery-recovery/outage](data/delivery-recovery/outage/).

**Smoke tests.** Two smoke tests with a 20 s outage preceded the run; they are not trials.

- The first exited 1. The frozen set lacked the `spool_dump` example that the harness calls, so it measured nothing. That set was rebuilt with every example the harnesses use.
- The second passed. It is kept under [smoke](data/delivery-recovery/outage/smoke/).

**Other load on the host.** Other work ran on the host during parts of the trials:

- Trial 1's outage phase (15:51–16:21 UTC), with the server stopped: a 2-minute soak-harness smoke test, the fast check profile, a toolchain download and a container image build.
- Trial 3, 16:58:16–17:00:33: a package build, which overlapped the last 34 s of its first minute and the start of its outage.

No other work ran during any drain phase.

## Results

| Seed | Drain s | Backlog at restart (batches) | Lines offered | Lines committed | Peak Spool bytes | Node VmHWM KiB | Trial s |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 99.0 | 1,803 | 4,044 | 4,044 | 3,725,504 | 5,220 | 2,025 |
| 2 | 97.9 | 1,803 | 4,042 | 4,042 | 3,725,504 | 5,284 | 2,024 |
| 3 | 97.9 | 1,803 | 4,042 | 4,042 | 3,725,504 | 5,284 | 2,024 |

| Gate | Rule | Result |
| --- | --- | --- |
| Drain | at most 600 s | passed: the largest drain was 99.0 s |
| Exactness | every offered line committed on the node; the delivery oracle passes over the Spool, the node's delivery lines and `server_dump` | passed in all three; 0 violations |
| Memory | node VmHWM at most 64 MiB | passed: the largest was 5.2 MiB |
| Exits | both processes exit 0 | passed |

## Interpretation

The drain runs at about 18 batches per second. That is what one-in-flight delivery gives with about 54 ms per acknowledged batch (grouped commit plus loopback TLS), so the drain time is set by the in-order delivery rule of [ADR-0013](../../decisions/ADR-0013-deliver-batches-in-order-with-bounded-dedup.md), not by the server. At the registered rate, a 30-minute outage leaves 1,803 batches and 3.7 MB (3.6 MiB) in the Spool, far below its 256 MiB limit.

The three seeds give nearly identical numbers because the workload differs only in line content.

## Limits

- The outage is a stopped server process, not a network partition.
- The run uses one node, loopback TLS and one host.
- A Spool that fills during an outage is not exercised here; the verification matrix records it under SPOOL-1.
