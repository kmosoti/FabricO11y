# Milestone: Linux installation qualification

Status: complete and merged into `main` (`71349fc`); CI green on `c08b96d` (Rust, Documentation, Extended verification). Base: the delivery-and-recovery milestone (`e89c965`).

This milestone registers the running-installation acceptance list of the [installation contract](../PRODUCT-CONTRACT.md#linux-installation-contract) as checks with pass rules, then runs it in a disposable Debian 13 systemd container. The result is **Inconclusive**: the only host available has a legacy cgroup hierarchy, so `MemoryHigh` enforcement cannot be shown. The milestone changes no unit, directive, default, package script or product code.

## Acceptance criteria

| ID | Criterion | Deciding evidence |
| --- | --- | --- |
| LI-1 | Every item of the contract's acceptance list maps to a check with a pass rule, registered before any run in its own commit | the [protocol](../experiments/formal/installation-acceptance-protocol.md) and `tools/qualification/install/` (`bbd2dd5`) |
| LI-2 | The checks run on a disposable systemd host, never on a real one, and the host is recorded | `run.sh` starts and removes a privileged container; the run record's environment section |
| LI-3 | The package is built with the pinned toolchain, and its hash is recorded | `RUSTUP_TOOLCHAIN=1.98.0 packaging/build-deb.sh`; [package.sha256](../experiments/benchmarks/data/linux-installation/package.sha256) |
| LI-4 | Representative packaging defects are rejected: the node as root, a missing collision check, a missing `MemoryMax` | three `--mutate` runs, each failing its named check |
| LI-5 | Anything the environment cannot show is reported as not run, and the overall state follows the protocol's decision rule | the A8h line; the state Inconclusive |
| LI-6 | The records agree | the capability ledger, verification matrix, deployment view, static packaging record, experiments index, roadmap, current state and the contract's status line; docs check exit 0 |

The milestone requires a recorded, honestly stated result. It does not require a pass, which this environment cannot give.

Out of scope:

- **A unified-hierarchy host.** None is available. Switching this VM would dismantle the platform's own v1 `memory` and `pids` controls, and it has no KVM for a nested VM.
- **Upgrades from an earlier package version.**
- **Reboots.**

## Definition of done

LI-1 to LI-6 are met, with commands and exits recorded below. The PR's CI is green (Rust, Documentation, Extended verification), and the PR is merged into `main`.

## Results

| Criterion | Command | Exit | Result |
| --- | --- | --- | --- |
| LI-1 | commit `bbd2dd5` | — | Checks A1 to A13b and A8h registered, with pass rules and a decision rule (Pass, Inconclusive or Failed). Three development runs came first; they fixed the harness's handling of the legacy hierarchy and are not results |
| LI-3 | `RUSTUP_TOOLCHAIN=1.98.0 packaging/build-deb.sh target/alpha-install-deb` | 0 | `59bfd745…8344b`, `SOURCE_DATE_EPOCH` 1790623428 |
| LI-2, LI-5 | `tools/qualification/install/run.sh <deb> <out>/clean` | 3 | **Inconclusive.** 16 of the 17 checks passed: sysusers, collision refusal, repeated install, verify, TLS, the non-root identity, directories, cgroup placement and kernel limits, sandboxed metrics and the authorized log, the denied-log gap, restart with a retained Spool, memory and task containment, remove and purge. A8h `MemoryHigh` was not run (legacy hierarchy) |
| LI-4 | `run.sh … --mutate root-user` | 1 | A6, A7 and A10 failed |
| LI-4 | `run.sh … --mutate no-collision-check` | 1 | A2a, A2b and A3 failed |
| LI-4 | `run.sh … --mutate no-memory-max` | 1 | A8 failed |
| LI-6 | `bun tools/docs/check.mjs` | 0 | Documentation checks passed |

Details are in [installation acceptance run 01](../experiments/formal/installation-acceptance-run-01.md).

State after this milestone:

- **Running installation:** Inconclusive.
- **What decides it:** a run on a host with the unified cgroup hierarchy.
