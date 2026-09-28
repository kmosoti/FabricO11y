# Registered running-installation acceptance

Status: registered on 2026-09-28 in the Linux-installation milestone, before any acceptance run. Three development runs of the harness preceded this registration; they fixed the harness and are not results. Results go in a separate run record. This protocol turns the acceptance list of the [installation contract](../../PRODUCT-CONTRACT.md#linux-installation-contract) into checks with pass rules. It changes no unit, directive or default.

## Environment

The checks run as root inside a **disposable Debian 13 container with systemd 257 as PID 1**, built from [`tools/qualification/install/Dockerfile`](../../../tools/qualification/install/Dockerfile). The container runs with `--privileged --cgroupns=private`.

The host's kernel, CPUs and memory are shared, so the container is not a separate machine. What the checks establish is behavior under systemd and the kernel's cgroup controllers. [`run.sh`](../../../tools/qualification/install/run.sh) removes the container afterward.

**Unified versus legacy hierarchy.** The contract assumes that systemd owns the unified cgroup hierarchy (cgroup v2).

On a host whose `/sys/fs/cgroup` is not unified, `run.sh` boots systemd 257 in forced legacy mode (`systemd.unified_cgroup_hierarchy=0 SYSTEMD_CGROUP_ENABLE_LEGACY_FORCE=1`), and `acceptance.sh` reads the v1 files instead:

- `MemoryMax` is `memory.limit_in_bytes`;
- `TasksMax` is `pids.max`.

`MemoryHigh` has no v1 counterpart, so its enforcement (A8h) is **not run** on such a host.

The package is built by [`packaging/build-deb.sh`](../../../packaging/build-deb.sh) with the pinned toolchain.

## Checks

[`acceptance.sh`](../../../tools/qualification/install/acceptance.sh) runs the following checks in order. It prints one line per check and exits with one of these statuses:

- 0: every check ran and passed;
- 1: a check failed;
- 3: no check failed, but a check could not run in this environment.

| ID | Contract item | Pass rule |
| --- | --- | --- |
| A1 | sysusers dry run and temporary root | The dry run exits 0, names `fabricolly` and writes nothing. Two real applies to a temporary root produce the same line `fabricolly:x:<uid>:<gid>:Fabric O11y service:/:/usr/sbin/nologin`. |
| A2a | existing-account collision refusal | With a human account `fabricolly` (UID 1500, login shell) present, `dpkg -i` fails with the refusal message and unpacks nothing. |
| A2b | the same, for the group | With a non-system group `fabricolly` (GID 1600) present, `dpkg -i` fails with the refusal message and unpacks nothing. |
| A3 | repeated install | `dpkg -i` twice both exit 0 and give the same account: a system UID (1 to 999), primary group `fabricolly`, shell `/usr/sbin/nologin`. |
| A3b | configuration directory | `/etc/fabrico11y` is `0750 root:fabricolly`. |
| A4 | `systemd-analyze verify` | Exit 0 on the installed node service, server service and slice. |
| A5 | TLS | The admin API over TLS rejects a client that does not trust the private CA (curl exit 60) and answers 200 to one that does. The node delivers only over TLS (A9). |
| A6 | actual non-root UID and primary GID | Both running services have the `fabricolly` UID and GID, not 0, with `CapEff` 0 and `NoNewPrivs` 1. |
| A7 | managed directories | `/var/lib/fabrico11y/{node,server}` and `/run/fabrico11y/{node,server}` are `0700 fabricolly:fabricolly`. |
| A8 | cgroup placement, effective limits and accounting | See the detail below. |
| A8h | `MemoryHigh` enforcement | On the unified hierarchy: the node cgroup's `memory.high` is 128M and the slice's is 2816M. On the legacy hierarchy: not run. |
| A9 | metric reads and one authorized log inside the sandbox | Through TLS delivery and the query API: `system.memory.available` points arrive, and a line from a log readable only through `setfacl u:fabricolly:r` arrives. |
| A10 | visible gap for a denied log | A `0600 root` log without an ACL produces a gap `log source unavailable …denied.log…`, and its content is never delivered. |
| A11 | graceful shutdown and restart with a retained Spool | See the detail below. |
| A12 | memory and task pressure containment | See the detail below. |
| A13a | uninstall keeps user data | `dpkg -r` exits 0 and removes the binaries. The services are no longer active. `/var/lib/fabrico11y`, `/etc/fabrico11y` and the account remain. |
| A13b | purge deletes it | `dpkg -P` exits 0, and `/var/lib/fabrico11y` and `/etc/fabrico11y` are gone. |

**A8 detail.**

- Each service's cgroup is `/system.slice/system-fabrico11y.slice/<unit>`.
- `EffectiveMemoryHigh`, `EffectiveMemoryMax` and `EffectiveTasksMax` are the frozen defaults: node 128M, 256M and 128; server 2560M, 3072M and 512.
- The kernel's memory limit and `pids.max` for each service cgroup equal the unit's `MemoryMax` and `TasksMax`.
- The slice's kernel memory limit and `pids.max` are 3328M and 640.
- `MemoryCurrent` and `TasksCurrent` are set (accounting is on).
- I/O accounting and weights are reported and never counted as enforcement.

**A11 detail.** The check runs these steps in order:

1. Stop the server; its `Result` must be `success`.
2. Append a line to the authorized log while the server is down.
3. Restart the node with SIGTERM; its `Result` must be `success`, and the Spool must hold files.
4. Start the server again.

The check passes when the outage line arrives within 90 s and the earlier line is still answered.

**A12 detail.** The check runs these steps inside `system-fabrico11y.slice`:

1. It starts a transient unit that allocates memory without bound (`tail /dev/zero`) under the node's own limits (`MemoryHigh=128M`, `MemoryMax=256M`, `TasksMax=128`).
2. It starts a second hog that is bounded only by the slice.
3. It starts a `bash` process tree that tries to start 300 tasks under `TasksMax=128`. Sampled `TasksCurrent` must peak between 100 and 128, and fork failures must be logged.

For each hog, `MemoryCurrent` is sampled every 0.2 s and must never exceed the limit, and the kernel must kill the hog:

- on the unified hierarchy, `Result=oom-kill`;
- on the legacy hierarchy, where systemd cannot observe OOM events, `Result=signal` with status 9 before `RuntimeMaxSec=300`.

The check passes when, after all three, both services are active with unchanged `NRestarts` and a new log line is still delivered.

## Negative controls

`run.sh --mutate <name>` injects one packaging defect before the run. Each must fail at least its named check:

| Mutation | Defect | Must fail |
| --- | --- | --- |
| `root-user` | the node unit runs as `User=root` | A6 |
| `no-collision-check` | `preinst` accepts any existing account | A2a, A2b |
| `no-memory-max` | the node unit loses `MemoryMax` (the kernel limit becomes unlimited) | A8 |

## Decision rule

**Pass.** The running installation passes when both of these hold:

- the unmutated package passes every check (exit 0) on a unified-hierarchy host;
- each mutation fails its named check.

**Inconclusive.** On a legacy-hierarchy host with no check failed (exit 3), the result is **Inconclusive**. Everything else is shown, but `MemoryHigh` enforcement and the unified-hierarchy behavior the contract assumes are not.

**Failed.** Any failed check is **Failed**.

Report the host kernel, the systemd version and the cgroup controllers. A container run is recorded as such, not as a bare-metal or VM installation. Anything the environment cannot show is recorded as not shown.

## Limits

- **Host sharing.** The container shares the host kernel. Its hostname, `/proc` and `/sys` views are the container's.
- **Pressure tests.** They use transient units in the slice, not the daemons themselves under load.
- **Not covered.** Power loss, reboots and upgrades from an earlier package version are not exercised.
