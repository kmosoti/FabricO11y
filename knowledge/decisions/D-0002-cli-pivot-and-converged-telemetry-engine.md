---
id: D-0002
title: CLI Pivot, Sans-I/O Separation, and Converged Pure-Rust Telemetry Engine
type: decision
status: accepted
date: 2026-11-14
added_by: agent
human_ratified: false
related:
  - D-0001
  - O-0001
  - R-0001
---

# D-0002 · CLI Pivot, Sans-I/O Separation, and Converged Pure-Rust Telemetry Engine

## Decision

Pivot `FabricO11y` from UI/external-DB prototypes into a **single-binary, zero-UI Linux host + agent + OTLP observability sensory plane**:

1. **Zero UI Scope:**
   - No HTML/CSS/JS/WASM web UI crates (`fabric-view`) or HTTP dashboard servers belong in `FabricO11y`.
   - All human and autonomous consumers (`agy`, `Metron`, scripts, operators) use the deterministic `fabric` CLI (`fabric-cli`) emitting `--json` (`schema_version = "fabric.cli.v1"`, automatic default when stdout is non-TTY), `--arrow` (Arrow IPC stream), or `--table`.
2. **Zero External Database Daemons for Local Telemetry:**
   - Replace external GreptimeDB/ClickHouse/Postgres requirements with an embedded pure-Rust converged engine under `FABRIC_DATA_DIR=/data/fabrico11y`:
     - **OLTP & Hot Graph:** `redb` (`catalog.redb`) + atomic `live_state.json` snapshot mirror.
     - **Hot Time-Series:** `HotMetricRing` (`<= 32 MiB` RAM cap, Gorilla delta-of-delta + XOR bit packing, 60s Parquet flush).
     - **OLAP & Cold Graph:** ZSTD-compressed Arrow/Parquet partitions (`olap/{metrics,logs,traces,fs_events,edges}/dt=YYYY-MM-DD/*.parquet`) queried via memory-capped DataFusion (`64 MiB` `GreedyMemoryPool` + disk spill under `<data_dir>/state/spill`).
3. **Strict Sans-I/O Core Boundaries:**
   - `fabric-core` and `fabric-graph` (alongside verified `admission-kernel`) perform zero OS, filesystem, socket, or async runtime I/O.
   - `fabric-store`, `fabric-collect`, `fabric-query`, and `fabric-cli` isolate storage, `/proc` & `/sys/fs/cgroup` & `inotify` Linux collection, DataFusion planning, and terminal output.
   - Existing `fabric` SDK, `spindle`, `fabric-server` canonical custody, `fabric-mcp`, and `admission-kernel` formal verification contracts remain preserved.
4. **Hard Self-Preservation Invariants:**
   - Strictly reject `/tmp` and `/dev/shm` paths at runtime (`validate_non_tmp_path`).
   - Run `fabric daemon` inside `fabrico11y.slice` (`MemoryHigh=256M`, `MemoryMax=512M`, `CPUQuota=15%`, `IOWeight=30`, `TasksMax=64`) with a self-shedding `200 MiB` RSS watchdog and `fabric doctor --json` verifying `rss_bytes < 100 MiB`, `open_fds < 64`, and `tmp_files_created == 0`.

## Alternatives Considered

- **Embedded SQLite + external GreptimeDB sidecar:** Rejected because SQLite introduces C build toolchain coupling for unified OLTP/graph while external DB containers consume hundreds of megabytes of baseline RAM on the workstation being observed.
- **Browser/Tauri GUI:** Rejected per post-mortem evidence because local desktop sessions and autonomous agent loops need scriptable JSON/Arrow queries (`fabric blame`, `fabric lineage`), not extra GPU/RAM-heavy browser processes.

## Revisit When

- Multi-host federated aggregation across remote nodes outgrows local workstation custody (preserve `fabric-server` canonical receiver for cluster-level fan-in).

## Relations

- *informed-by*: [O-0001](../observations/O-0001-fedora-blade-resource-postmortem.md)
- *backed-by*: [R-0001](../research/R-0001-converged-rust-storage-and-five-pillars.md)
- *aligned-with*: [D-0001](D-0001-development-workflow.md)
