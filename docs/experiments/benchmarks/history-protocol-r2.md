# Registered history measurement, revision 2: four-CPU host

Status: registered on 2026-09-28 in the history-qualification milestone, before any run under it. It amends only the placement of processes on CPUs in [revision 1](alpha-phase4-history-protocol.md), which remains the registered protocol for the target profile. Results go in a separate run record.

## Why a revision

Revision 1 pins the server to logical CPUs 0 to 3 and the simulator to CPUs 4 and above; it was written for a 12-CPU host. The only host available in this milestone has four logical CPUs, where CPU 4 does not exist, so revision 1 cannot run as written. Changing a registered protocol after the fact would be a silent change; this revision is registered first instead.

## What changes

| Item | Revision 1 | Revision 2 |
| --- | --- | --- |
| Server CPUs | `taskset -c 0-3` | `taskset -c 0-1` |
| Simulator CPUs | `taskset -c 4-<last>` | `taskset -c 2-3` |
| Harness | `history_tier.py` as registered | the same script with two added arguments, `--server-cpus` and `--sim-cpus`, whose defaults reproduce revision 1 exactly |
| Host | 12 logical CPUs, Debian 13 / WSL2 | 4 logical CPUs, 15 GiB RAM, Ubuntu 24.04 in a Firecracker VM, ext4 |

Everything else is unchanged and binding: 1,000 identities; the frozen workload (two 512-byte log records per identity per second, 32 metric points every 15 s); 250 s; seeds `0xA11FA001` to `0xA11FA003` in segment mode (64 MiB journal files) and one journal-only trial with `0xA11FA001`; the freshness prober; the five registered query kinds, 20 instances each in a seeded order; grading by the frozen query oracle; the runner with a 1,800 s duration limit, 5 GiB live-data limit and 1 MiB evidence limit; and the decision rule: every graded answer passes the oracle, each query kind's p99 is at most 2 s, and freshness p99 is at most 5 s in each segment-mode trial. A budget stop or unrun trial is a failure.

## Consequences for interpretation

The server has two CPUs instead of four and shares the host with the simulator and the prober on the other two, so latency and freshness are measured under more contention than revision 1 intended. A pass under revision 2 shows the gates hold on this smaller host; it is **not** a target-profile qualification, which still requires revision 1 on the target host. A failure under revision 2 is a failure on this host, recorded as such, not dismissed as an environment artifact.

## Procedure

Build the release binaries and examples at the milestone head, copy them and the harness into `target/alpha-p4r2-frozen` with SHA-256 values recorded, and run each trial sequentially:

```sh
python3 -B tools/qualification/runner.py --out target/alpha-p4r2-<mode>-seed<N> --duration-s 1800 \
  --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
  python3 -B target/alpha-p4r2-frozen/tools/history_tier.py --seed 0xA11FA00<N> --mode <mode> \
  --bin-dir target/alpha-p4r2-frozen --server-cpus 0-1 --sim-cpus 2-3
```

## Erratum (before any measurement)

The runner starts its child with the owned output directory as the working directory, so the relative paths in the command above cannot resolve. The first attempt at 2026-09-28T15:07:49Z started all four trials with those paths; each child exited 2 within 0.03 s, before starting a server, and nothing was measured. The procedure uses absolute paths to the frozen harness and binaries instead:

```sh
python3 -B tools/qualification/runner.py --out target/alpha-p4r2-<mode>-seed<N> --duration-s 1800 \
  --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
  python3 -B "$PWD/target/alpha-p4r2-frozen/tools/history_tier.py" --seed 0xA11FA00<N> --mode <mode> \
  --bin-dir "$PWD/target/alpha-p4r2-frozen" --server-cpus 0-1 --sim-cpus 2-3
```

Nothing else changes.
