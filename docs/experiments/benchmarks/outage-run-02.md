# Outage buffering and drain, run 02 (target profile)

Status: the three trials of the registered [outage protocol](alpha-phase5-outage-protocol.md) ran on 2026-09-28 and 2026-09-29 on the 12-CPU target host and each passed every gate. The outage-and-drain capability is **Qualified** on the target profile for commit `e68d6ce`.

## Method

As registered, with no placement: one `fabric-server` and one enrolled `fabric-node run` offered 2 lines/s of 512-byte bodies and sampling host metrics every 15 s; the server ran 60 s, was stopped for 1,800 s, then restarted; drain time runs from the restart until the node's acknowledged sequence reaches the last sequence committed at the restart. The artifacts are the set frozen from `e68d6ce1364ad02f2dfaa86e27922d3b8627155b` described in [history run 02](history-run-02.md), on the same [host](data/target-qualification/alpha-q1-host.txt).

For each seed `N`:

```sh
python3 -B tools/qualification/runner.py --out target/alpha-q1-outage-seedN --duration-s 3000 \
  --disk-bytes 1073741824 --max-output-bytes 1048576 -- \
  python3 -B $F/tools/outage_drain.py --seed 0xA11FA00N --bin-dir $F
```

All three runner invocations exited `0` with `passed=true` and no stop reason; each took about 2,040 s. Start and end times are in [progress.txt](data/target-qualification/outage/progress.txt); runner results and summaries are in [data/target-qualification/outage](data/target-qualification/outage/).

## Results

| Seed | Batches buffered at restart | Drain s | Lines offered / committed | Peak spool bytes | Node VmHWM KiB |
| ---: | ---: | ---: | --- | ---: | ---: |
| 1 | 1,802 | 114.1 | 4,074 / 4,074 | 4,112,259 | 5,240 |
| 2 | 1,803 | 114.1 | 4,074 / 4,074 | 4,114,039 | 5,160 |
| 3 | 1,802 | 114.0 | 4,073 / 4,073 | 4,112,259 | 5,044 |

Every gate was true in every trial: drain at most 600 s, every offered line committed, the delivery oracle exact over the node spool, the node's delivery lines and `server_dump`, node VmHWM at most 64 MiB, and both processes exited 0.

## Interpretation

A 30-minute outage left about 1,800 batches and 4 MiB in the node spool, and the node delivered them in about 114 s after the server returned, about a fifth of the 600 s limit, while it kept collecting. The three drain times agree to 0.1 s, which fits a node that sends one batch at a time at a steady round-trip cost. [Outage run 01](outage-run-01.md) drained in at most 99 s on a four-CPU host; the difference was not investigated.

## Limits

One host and loopback TLS; the outage is a stopped server process, not a network partition. Three trials.
