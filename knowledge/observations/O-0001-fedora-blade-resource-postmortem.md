---
id: O-0001
title: fedora-blade 13.2-Day Resource Exhaustion Post-Mortem and Telemetry Mapping
type: observation
status: observed
date: 2026-11-14
added_by: agent
human_ratified: false
related:
  - D-0002
  - R-0001
---

# O-0001 · `fedora-blade` 13.2-Day Resource Post-Mortem

## Observed Incident Telemetry (`fedora-blade`)

Four failure modes combined during an unbounded 13.2-day autonomous agent desktop session before host remediation:

1. **Silent `/home` Storage Exhaustion (`286 GB -> 160 GB`, `126 GB` reclaimed):**
   - `uv` cache grew to `~19 GB` alongside abandoned model weights (`~71 GB` in `~/.cache/metron-*` / `~/.cache/huggingface/`), `8.7 GB` Cargo `target/` sprawl, and `~14.4 GB` container/overlay artifacts.
   - *Host fix applied:* Routed `UV_CACHE_DIR`, `HF_HOME`, and `CARGO_BUILD_TARGET_DIR` to `/data` (`/data/cargo-target`, `/data/tmp`, `/data/fabrico11y`).
2. **RAM-Backed `/tmp` (`tmpfs`) and `zram0` Swap Saturation (`11 GB` `/tmp` + `8.0 GiB` full swap):**
   - Fedora mounts `/tmp` on a `16 GiB` in-memory `tmpfs`. Leftover `/tmp/fabric-rust-1100` (`8.3 GB`) plus 17 scratch directories (`~1.7 GB`) and 1,245 unlinked open FDs pushed `7.3 GB` into `/dev/zram0`, leaving `104 KiB` swap free and triggering `low-memory-monitor`.
3. **Unbounded Cgroup Memory & Coredump Amplification (`28.7 GB` peak on `user.slice`):**
   - Observed peaks across `codex-remote.service` (`24.1 GB` / `19.6 GB`), `metron-pool-*` (`16.8 GB`), `ptyxis-spawn-*` (`12.3 GB`), `libpod-*` (`8.3 GB`), and `systemd-coredump`/`abrtd` (`6.0–6.2 GB` dump amplification).
   - *Host fix applied:* Dedicated `agent.slice` (`MemoryHigh=14G`, `MemoryMax=18G`), capped `app.slice` (`MemoryHigh=22G`, `MemoryMax=26G`), and clamped coredumps to `512M`.
4. **Orphaned Systemd Units & Zombie Daemons (30 failed units):**
   - Transient `systemd-run` units (`metron-coding-*`, `fabrico11y-*`) omitted `--collect` (`CollectMode=inactive-or-failed`), pinning unit memory in `systemd --user`, alongside forgotten preview services running for 13.2 days.

## Mapping to `FabricO11y` Requirements

| Observed Failure | Required Collector | CLI Query / Attribution |
| --- | --- | --- |
| `/tmp` (`tmpfs`) + `/data` / `/home` churn | `fs_watch` (`inotify` + bounded `O_NOFOLLOW` scan) + `/proc/<pid>/fd` unlinked FD tracker | `fabric blame tmpfs --json`, `fabric blame disk:/data --json`, `fabric query fs` |
| Cgroup & agent RAM spikes | `cgroups` walker (`/sys/fs/cgroup` + `/proc/<pid>/{stat,statm,status,cmdline}`) | `fabric blame memory --json`, `fabric query agents` |
| PSI & `zram0` thrashing | `host` collector (`/proc/pressure/*`, `/sys/block/zram0/mm_stat`, `/proc/swaps`) | `fabric blame swap --json`, `fabric drift --pillar distribution` |
| Transient units without `--collect` & failed units | `systemd` collector (`$XDG_RUNTIME_DIR/systemd/transient/*` + `cgroup.events`) | `fabric query units --json`, `fabric drift --pillar lineage` |
| Cross-layer blame correlation | `fabric-graph` `CausalGraph` | `fabric lineage <target> --direction both --json` |

## Unknowns & Limits

- Root-owned kernel/system units outside user read permissions depend on readable `/sys/fs/cgroup` and `/proc` attributes; unprivileged user sessions cannot force cleanup of unrelated slices and inspect rather than mutate external processes.

## Relations

- *motivates*: [D-0002](../decisions/D-0002-cli-pivot-and-converged-telemetry-engine.md)
- *informs*: [R-0001](../research/R-0001-converged-rust-storage-and-five-pillars.md)
