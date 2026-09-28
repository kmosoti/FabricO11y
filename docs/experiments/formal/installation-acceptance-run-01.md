# Running-installation acceptance, run 01

Status: the registered [installation acceptance](installation-acceptance-protocol.md) ran on 2026-09-28. The result is **Inconclusive**: 16 of the 17 checks ran and passed, and each of the three negative-control mutations failed its named checks. One check did not run: A8h, `MemoryHigh` enforcement, because the host has only a legacy cgroup hierarchy.

This is the first running installation of the package. It is not a pass: the contract assumes that systemd owns the unified hierarchy, and that was not available.

## Environment

- **Host.** A Firecracker VM with 4 CPUs and 15 GiB RAM, running kernel `6.18.44-fc-v37` and Docker (cgroupfs driver). The host has no KVM, so a nested VM was not possible.
- **Cgroups.** `/sys/fs/cgroup` is a tmpfs of v1 hierarchies. The platform that runs this VM keeps its own processes in the v1 `memory` and `pids` hierarchies (`process_api`, `sbx-telemetry`). Switching the VM to the unified hierarchy would have meant dismantling those controls, so it was not done.
- **Container.** The container runs Debian 13.7 with systemd 257 (257.13-1~deb13u1) as PID 1, booted in forced legacy mode. `systemctl is-system-running` reported `degraded`: only `systemd-modules-load.service` failed, because a container cannot load kernel modules.

## Method

The package was built with `RUSTUP_TOOLCHAIN=1.98.0 packaging/build-deb.sh target/alpha-install-deb` at the registration commit, `bbd2dd5`. The pinned rustc 1.98.0 was used, and `SOURCE_DATE_EPOCH` 1790623428 is that commit's time. The package's SHA-256 is `59bfd745a9f24e4ef9033e1de22c1b68c0a365248e6d48997d9866597ab8344b`. The four invocations below ran from 19:24:12 to 19:28:38 UTC:

```sh
tools/qualification/install/run.sh target/alpha-install-deb/fabrico11y_0.1.0~alpha.1_amd64.deb <OUT>/clean
tools/qualification/install/run.sh <same package> <OUT>/<mutation> --mutate <mutation>   # root-user, no-collision-check, no-memory-max
```

The output of each invocation, and the unmutated run's service journal, are in [data/linux-installation](../benchmarks/data/linux-installation/package.sha256).

## Results

**Unmutated package: exit 3.** No check failed, and one could not run.

| Check | Result | Evidence |
| --- | --- | --- |
| A1 sysusers | passed | The dry run wrote nothing. Two applies to a temporary root gave `fabricolly:x:999:999:Fabric O11y service:/:/usr/sbin/nologin` |
| A2a human-account collision | passed | With UID 1500 present, `dpkg -i` refused and unpacked nothing |
| A2b non-system-group collision | passed | With GID 1600 present, `dpkg -i` refused and unpacked nothing |
| A3 repeated install | passed | Two installs gave the same account, `fabricolly:x:996:996:…:/usr/sbin/nologin` |
| A3b `/etc/fabrico11y` | passed | `750 root:fabricolly` |
| A4 `systemd-analyze verify` | passed | exit 0 |
| A5 TLS | passed | An untrusted client got exit 60; a CA-verified client got 200 |
| A6 identity | passed | Both services ran with UID/GID 996, `CapEff` 0 and `NoNewPrivs` 1 |
| A7 managed directories | passed | All four directories were `700 fabricolly:fabricolly` |
| A8 placement and limits | passed | Both units sat in `/system.slice/system-fabrico11y.slice/<unit>`. Effective limits were 128M/256M/128 (node) and 2560M/3072M/512 (server). The kernel limits `memory.limit_in_bytes` and `pids.max` were equal to them; the slice's were 3328M and 640. Accounting values were set |
| **A8h `MemoryHigh`** | **not run** | The legacy hierarchy has no `memory.high`; systemd reports the value only |
| A9 metrics and the authorized log | passed | `system.memory.available` points arrived, and so did the ACL-authorized line, both over TLS |
| A10 denied log | passed | The gap read `log source unavailable /var/log/fabric-accept/denied.log: Permission denied (os error 13)`; the log's content was never delivered |
| A11 restart with a retained Spool | passed | The server stopped with `Result=success`. The node restarted with `Result=success`, holding 5 Spool files. The outage line arrived after the restart, and the earlier line was kept |
| A12 pressure containment | passed | The hog under the service limits peaked at 268,435,456 bytes and was SIGKILLed. The hog under the slice limit peaked at 3,477,590,016 bytes and was SIGKILLed. The task tree peaked at `TasksCurrent`=128 with fork failures logged. Both services stayed active with `NRestarts` unchanged, and delivery continued |
| A13a remove | passed | The binaries were gone and the services inactive. State, configuration and the account were kept |
| A13b purge | passed | `/var/lib/fabrico11y` and `/etc/fabrico11y` were removed |

**Negative controls: every mutation exited 1 and failed at least its named check.**

| Mutation | Checks failed | Named check |
| --- | --- | --- |
| `root-user` | A6 (node UID 0), A7 (node directories owned by root), A10 (no denied-log gap appeared, as expected when root can read the file) | A6 ✓ |
| `no-collision-check` | A2a and A2b (installed over the colliding account and group), A3 (UID 1600, not a system UID) | A2a, A2b ✓ |
| `no-memory-max` | A8 (the node's effective memory limit became the slice's 3,489,660,928 bytes) | A8 ✓ |

The `root-user` result is worth noting. It shows concretely that the non-root identity is what makes the denied-log check meaningful: under root, the unauthorized log no longer produces a gap.

## Interpretation

On Debian 13 with systemd 257, the package does what the contract says:

- it installs, refuses collisions and verifies;
- it runs unprivileged in the slice with its limits in the kernel;
- it collects inside the sandbox, keeps its Spool across restarts, and contains memory and task pressure;
- it uninstalls without losing data unless purged.

Two things are not shown:

- `MemoryHigh` throttling;
- behavior under a unified hierarchy that systemd owns, which the contract assumes.

A run on a unified-hierarchy host decides the gate.

## Limits

- **Environment.**
  - The container shares the host kernel. It is not a separate machine.
  - OOM kills are inferred from SIGKILL, because systemd cannot observe OOM events on v1.
  - I/O accounting (`IOReadBytes`) is not available on this hierarchy; it is reported and never counted.
- **Coverage.** Upgrades from an earlier package version, reboots and power loss are not exercised.
