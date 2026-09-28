# Registered soak measurement

Status: registered on 2026-09-28 in the delivery-and-recovery milestone, before any soak trial. Results go in a separate run record.

## Question

Does one `fabric-server` stay correct and bounded while it runs for about 90 minutes with fleet delivery, sealing, queries and management all happening at once? Bounded means ACK latency does not degrade, the backlog does not grow, and memory does not grow.

## Fixture and workload

[`soak_tier.py`](../../../tools/qualification/soak_tier.py) starts one `fabric-server`. The fleet [simulator](../../../examples/spindle_sim.rs) drives **100 enrolled identities** with the frozen workload: each identity offers two 512-byte log records per second and 32 metric points every 15 s, using 100 workers, one per identity. The run lasts **5,460 s**: a 60 s warm-up, then nine measured windows of 600 s each.

The server seals journal files at 64 MiB into Segments, so sealing runs throughout. About 700 MB of journal are expected, which is about ten Segments. Retention is left at its defaults (24 h and 20 GiB), which this run does not reach.

The size is chosen so that one invocation fits the runner's 2 h limit. That includes the final dump and oracle over about 546,000 batches. It also keeps the simulator's transcript and the server's data under the 5 GiB live-data limit. A 1,000-identity soak would exceed that limit, so it needs retention-aware grading and is not registered here.

While the simulator runs, the harness keeps up three concurrent activities:

- **Queries.** Every 10 s, a log query for one random identity over the last 30 s, limit 100, following every page. Its latency and each page's `complete` flag are recorded.
- **Management.** Every 60 s, an inventory listing and a configuration change for one random identity.
- **Sampling.** Every 5 s, the server's VmRSS and CPU time.

After the simulator ends, the harness:

1. waits up to 300 s for every sealed journal file to become a Segment;
2. stops the server with SIGTERM;
3. grades the simulator's transcript together with `server_dump` using the frozen [delivery oracle](../../../tools/qualification/DELIVERY_ORACLE.md). Batch bytes are projected to SHA-256 on both sides.

## Placement and runner

- **Placement.** The server runs on logical CPUs 0 and 1 (`--server-cpus 0-1`) and the simulator on CPUs 2 and 3 (`--sim-cpus 2-3`); the harness is not pinned. This is the four-CPU placement of [history revision 2](history-protocol-r2.md). On the 12-CPU target host the placement is `0-3` and `4-11`, and that is a separate trial.
- **Trial.** One trial, with seed `0xA11FA001`.
- **Runner.** The frozen [runner](../../../tools/qualification/runner.py), with a 7,200 s duration limit, a 5 GiB live-data limit and a 1 MiB evidence limit.

## Decision rule

The trial passes when every gate in `soak_tier.evaluate` holds. [`test_soak_tier.py`](../../../tools/qualification/test_soak_tier.py) checks that function's negative controls: each injected defect fails exactly its own gate.

| Gate | Rule |
| --- | --- |
| `oracle` | the delivery oracle passes |
| `exits` | the simulator and the server both exit 0 |
| `ack_p99_le_1s_every_window` | ACK p99 is at most 1 s in each of the nine windows, and each window has samples |
| `no_growing_backlog` | the backlog 5 s before the end is at most one batch per identity above the backlog at the end of the warm-up |
| `server_rss_le_2gib` | server VmHWM is at most 2 GiB |
| `no_rss_growth` | median server RSS in the last window is at most twice the median in the first window plus 64 MiB |
| `queries_complete` | every query succeeds and every page is complete |
| `query_p99_le_2s` | query p99 over the run is at most 2 s |
| `management_ok` | every management call succeeds, and at least one ran |
| `sealing_caught_up` | no sealed journal file remains 300 s after the simulator ends, and at least one Segment exists |

A budget stop or an unrun trial is a failure. The run record reports, for each window, ACK p50 and p99, RSS and server CPU; it also reports backlog, query latency, live bytes and the Segment count.

## Procedure

Freeze the binaries, examples and harness with SHA-256 values at the milestone head, then run:

```sh
python3 -B tools/qualification/runner.py --out target/alpha-soak-seed1 --duration-s 7200 \
  --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
  python3 -B $PWD/target/<frozen>/tools/soak_tier.py --seed 0xA11FA001 \
  --bin-dir $PWD/target/<frozen> --server-cpus 0-1 --sim-cpus 2-3
```

`--smoke` shortens the run to two 30 s windows after a 10 s warm-up, to try out the harness. It is never a trial, and its summary records `"smoke": true`.

## Limits

- **Environment.** One host, loopback TLS, and simulated identities.
- **Duration.** About 90 minutes shows the absence of fast degradation, not long-term stability.
- **Retention.** Retention does not act at this size. Retention at scale stays an open item in the [verification matrix](../../formal/verification-matrix.md) (HIST-5).
