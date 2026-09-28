# Registered stress measurement, revision 2: four-CPU host

Status: registered on 2026-09-28 in the delivery-and-recovery milestone, before any stress run. It changes only where processes run on CPUs; everything else in [revision 1](alpha-phase5-stress-protocol.md) is unchanged. Revision 1 remains the registered protocol for the target profile. Results go in a separate run record.

## Why a revision

Revision 1 pins the server to logical CPUs 0 to 3 and the simulator to CPUs 4 and above. The only host available in this milestone has four logical CPUs, so the simulator's CPU list (`4-3`) does not exist and revision 1 cannot run as written. This follows the same reasoning as [history revision 2](history-protocol-r2.md).

## What changes

| Item | Revision 1 | Revision 2 |
| --- | --- | --- |
| Server CPUs | `taskset -c 0-3` | `taskset -c 0-1` |
| Simulator CPUs | `taskset -c 4-<last>` | `taskset -c 2-3` |
| Harness | `stress_tier.py` as registered | the same script with two added arguments, `--server-cpus` and `--sim-cpus`, whose defaults reproduce revision 1 exactly; the summary records the placement |
| Host | 12 logical CPUs, Debian 13 / WSL2 | 4 logical CPUs, 15 GiB RAM, Ubuntu 24.04 in a Firecracker VM, ext4 |

Everything else is unchanged and binding:

- **Workload.** 1,000 identities running the frozen workload for 180 s, with a 5× log burst from second 60 to second 79.
- **Rejection probes.** 200 rounds, each sending one batch under a revoked token, one under an unknown token, and one malformed body.
- **Concurrent management.** Every 0.5 s: an inventory, a configuration change, and a 30 s log query.
- **Seeds and runner.** Seeds `0xA11FA001` to `0xA11FA003`. The runner has a 1,500 s duration limit, a 5 GiB live-data limit and a 1 MiB evidence limit.
- **Decision rule.** A trial passes only if all of these hold:
  - the delivery oracle passes;
  - revoked and unknown tokens always get 401, and malformed bodies 400 or 413;
  - no batch from a rejected identity is committed;
  - the backlog 5 s before the end is at most one batch per identity above the backlog 5 s before the burst;
  - server VmHWM is at most 2 GiB;
  - every management call succeeds;
  - both processes exit 0.

  A budget stop or an unrun trial is a failure.

## Consequences for interpretation

The server has two CPUs instead of four. It shares the host with the simulator, the rejection prober and the management loop. Backlog recovery and management success are therefore tested under more contention than revision 1 intended.

A pass under revision 2 shows the gates hold on this smaller host. It is **not** a target-profile qualification. A failure under revision 2 is a failure on this host.

## Procedure

Build the release binaries and examples at the milestone head. Copy them and the harness into `target/alpha-p5r2-frozen`, recording their SHA-256 values. Then run each trial in turn, using absolute paths: the runner starts its child in the output directory.

```sh
python3 -B tools/qualification/runner.py --out target/alpha-p5r2-stress-seed<N> --duration-s 1500 \
  --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
  python3 -B $PWD/target/alpha-p5r2-frozen/tools/stress_tier.py --seed 0xA11FA00<N> \
  --bin-dir $PWD/target/alpha-p5r2-frozen --server-cpus 0-1 --sim-cpus 2-3
```
