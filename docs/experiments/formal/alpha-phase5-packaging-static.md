# Phase-5 packaging: reproducible build and static checks

Status: the static and unprivileged parts of plan steps 5.1 and 5.2 passed on 2026-09-28. The running-install acceptance in the [installation contract](../../ALPHA.md#linux-installation-contract-phase-5-not-yet-implemented) is **unrun**: it creates a system account and system units and needs root on a disposable systemd host. It must not be recorded as passed.

## What exists

[`packaging/`](../../../packaging/) holds the vendor sysusers file (`g fabricolly -`, then `u! fabricolly -:fabricolly "Fabric O11y service" - /usr/sbin/nologin`), `fabrico11y-node.service`, `fabrico11y-server.service` and `system-fabrico11y.slice` with exactly the contract's identity, directories, hardening and resource directives, example configurations, Debian maintainer scripts, and [`build-deb.sh`](../../../packaging/build-deb.sh). The `preinst` script refuses an existing `fabricolly` account or group that is not the expected non-login system identity before any file is unpacked. `postinst` runs `systemd-sysusers` and creates `/etc/fabrico11y` as `0750 root:fabricolly`. Remove keeps state, configuration and the account; purge removes `/var/lib/fabrico11y` and `/etc/fabrico11y`.

## Checks run

| Check | Command | Result |
| --- | --- | --- |
| Reproducible build | `CARGO_TARGET_DIR=target/package-{a,b} packaging/build-deb.sh target/alpha-p5-out-{a,b}`, two clean target directories | both exit 0; identical `.deb` SHA-256 `16e8d8af2172f5218b055a0acb0eccada65aaf966bb7b102f1526c29d9a6aec6` ([a](../benchmarks/data/alpha-phase5/build-a.sha256), [b](../benchmarks/data/alpha-phase5/build-b.sha256)) |
| sysusers dry run | `systemd-sysusers --dry-run --root=<empty root> packaging/sysusers.d/fabrico11y.conf` | exit 0; would create group and user `fabricolly` |
| sysusers temporary root | the same without `--dry-run`, then again | exit 0 both; `fabricolly:x:999:999:Fabric O11y service:/:/usr/sbin/nologin` and group `fabricolly:x:999:`; the second apply changed nothing |
| Collision refusal | `packaging/debian/preinst install` with stand-in `getent` and `id` | expected identity 0, absent 0, human UID 1001 → 1, wrong primary group → 1, non-system group GID 1005 → 1 |
| Unit syntax | `systemd-analyze verify --root=<host units + package>` on both services and the slice | exit 0 |
| Exposure | `systemd-analyze security --offline=true` | 4.8 ("OK") for both services |

The build used rustc 1.98.0, `--locked`, a `SOURCE_DATE_EPOCH` from the last commit and path remapping for the checkout and cargo home. The package holds the three binaries, the units and slice, the sysusers file and the examples, all owned by root.

## Not run

Every running-install item of the contract: installation on a live systemd host, actual non-root UID and primary GID of the running services, managed directory ownership and modes, cgroup placement and effective limits, sandboxed metric reads and an authorized log, denied-log gaps, TLS between installed services, graceful restart with retained spool, memory and task pressure containment, and uninstall with and without purge. These need a disposable host or VM with root.
