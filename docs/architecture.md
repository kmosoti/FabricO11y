# FabricO11y Architecture

`FabricO11y` is a **CLI-first, zero-UI Linux host, autonomous-agent, and OTLP observability sensory plane** built in pure Rust (Edition 2024, Rust 1.100.0). It combines strictly bounded live Linux telemetry collection (`cgroups` v2, `/proc` PSI/swap/zram0, systemd unit hygiene, `inotify`/fd filesystem churn, optional GPU sysfs, and local Spindle/OTLP ingest) with an embedded pure-Rust converged storage and causal attribution engine rooted under `/data/fabrico11y`.

## 1. Crate & Authority Boundaries

Pure domain logic, schema contracts, statistical sketches, causal graph traversal, and formal admission arithmetic live in **Sans-I/O** crates (`admission-kernel`, `fabric-core`, `fabric-graph`). All Linux kernel probing, embedded disk/memory custody, DataFusion query execution, and CLI rendering are isolated in dedicated I/O crates (`fabric-store`, `fabric-collect`, `fabric-query`, `fabric-cli`, plus preserved custody/protocol crates `fabric`, `fabric-server`, and `fabric-mcp`). Dependency boundaries are machine-verified by [`verification/architecture.toml`](../verification/architecture.toml) via `cargo xtask architecture`.

```mermaid
flowchart TD
    subgraph SansIO["Sans-I/O Core & Formal Verification"]
        AK["admission-kernel (Verus / Charon / Aeneas)"]
        FC["fabric-core (Models, PathPolicy, Gorilla HotMetricRing, UnifiedQuerySpec, DDSketch, EWMA-MAD, Five Pillars, Arrow)"]
        FG["fabric-graph (CausalGraph, BFS lineage, ranked blame)"]
        FC --> AK
        FG --> FC
    end

    subgraph StorageAndCollection["Embedded Storage & Linux Sensory Plane"]
        FS["fabric-store (redb OLTP/Graph, ZSTD Parquet, Janitor; re-exports HotMetricRing)"]
        COL["fabric-collect (Bounded mpsc try_send, cgroups v2, PSI/zram0, systemd, inotify/fd, Spindle bridge, SelfGuard)"]
        FS --> AK
        FS --> FC
        FS --> FG
        COL --> AK
        COL --> FC
        COL --> FG
        COL --> FS
        COL --> FAB
    end

    subgraph QueryAndCLI["Query Engine & Deterministic CLI"]
        FQ["fabric-query (Preserved Projection + DataFusion/Arrow Unified Engine)"]
        CLI["fabric-cli ('fabric' binary: query, custody, projection-ready)"]
        FQ --> FC
        FQ --> FG
        FQ --> FS
        FQ --> FAB
        CLI --> AK
        CLI --> FC
        CLI --> FG
        CLI --> FS
        CLI --> COL
        CLI --> FQ
        CLI --> FAB
    end

    subgraph PreservedCustody["Preserved Canonical Custody & Protocol Crates"]
        FAB["fabric (SDK & Spindle local custody)"]
        SRV["fabric-server (Canonical TCP ledger & reader)"]
        MCP["fabric-mcp (Typed stdio MCP adapter)"]
        FAB --> AK
        SRV --> FAB
        SRV --> FQ
        SRV --> AK
        MCP --> FAB
    end
```

## 2. Converged Telemetry Engine & Self-Preservation Pipeline

All collection and query execution obeys hard self-preservation limits:

- **Zero `/tmp` or `/dev/shm` usage:** Enforced statically in `fabric-core::paths::validate_non_tmp_path`, CI scripts, and `fabric doctor --json`.
- **Single local storage tree (`/data/fabrico11y`):**
  - `state/catalog.redb` + `state/live_state.json`: live entity registry, adjacency tables (`adj_out`/`adj_in`), unit hygiene records, watermarks (including `spindle_watermark_id`, persisted strictly **after** all OLAP Parquet domain flushes succeed), and recent anomalies.
  - `HotMetricRing` (in RAM, Sans-I/O in `fabric-core`): strictly capped at `32 MiB` with modular `wrapping` Gorilla delta-of-delta + XOR compression (preserving `labels_json`), buffer rollback on failed flush, and 60s Parquet flushes.
  - `olap/{metrics,logs,traces,fs_events,edges}/dt=YYYY-MM-DD/*.parquet`: ZSTD-compressed Arrow/Parquet partitions bounded by `RetentionJanitor` (`4 GiB` default cap, >7d downsampling).
- **Resource confinement (`fabrico11y.slice`):** `MemoryHigh=256M`, `MemoryMax=512M`, `CPUQuota=15%`, `IOWeight=30`, `TasksMax=64`, plus a `200 MiB` RSS self-shedding watchdog inside `fabric-collect`.
- **Formal proofs & fuzzing (`proofs/`, `fuzz/`):** Verus + Charon/Aeneas Lean 4 (`ExtractedAdmission.lean`), standalone Lean 4 (`Telemetry.lean`), Z3 `QF_BV` SMT (`TelemetrySmt.smt2`), and 8 `cargo-fuzz` targets (`gorilla_ring_roundtrip`, `unified_query_and_ipc`, `causal_graph_and_wire`, plus 5 core custody targets).

```mermaid
flowchart LR
    K["Kernel Sources (cgroups v2, /proc PSI, zram0, /proc/pid/fd)"] -->|"try_send (drop-with-counter)"| Q["Bounded mpsc Queues"]
    S["Systemd & FS Sources (transient units, inotify, /tmp, ~/.cache, /data/tmp)"] -->|"try_send (drop-with-counter)"| Q
    I["Local Ingest & Custody (Spindle custody poll + ingest.sock)"] -->|"bounded enqueue + backpressure"| Q

    Q --> PIPE["Daemon Collector Loop + SelfGuard (< 200 MiB Watchdog)"]
    PIPE --> EV["Five Pillars + DDSketch + EwmaMadDetector + CausalGraph"]

    EV --> RING["HotMetricRing (<= 32 MiB Sans-I/O Gorilla Ring)"]
    EV --> REDB["CatalogStore (redb + live_state.json + daemon.sock)"]
    RING -->|"60s Flush / Atomic Staging"| PQ["ZSTD Parquet Partitions (/data/fabrico11y/olap)"]
    PQ -->|"advance spindle_watermark_id post-flush"| REDB
    PQ --> JAN["RetentionJanitor (<= 4 GiB Cap + 7d Downsample)"]

    REDB --> QENG["fabric-query Unified Engine (DataFusion 64 MiB Pool)"]
    RING --> QENG
    PQ --> QENG
    REDB --> GRAPH["fabric-graph Lineage & Blame"]

    QENG -->|"snapshot / query / drift / doctor"| OUT["fabric CLI (--json | --arrow | --table)"]
    GRAPH -->|"lineage / blame"| OUT
    REDB -->|"custody status + server progress"| CUST["fabric custody (--json | --arrow | --table)"]
    I -->|"origin-qualified exact custody record"| READY["fabric projection-ready --json"]
    REDB -->|"origin-bound committed frontier"| READY
    PQ -->|"guarded full-row read and receipt provenance"| READY
```

The local projection binds its Spindle origin before staging a contiguous prefix.
New metrics, logs, and traces carry internal receipt provenance in Parquet. The
public query schema omits this field. Compaction retains raw provenance and metric
rollups discard it. The readiness check reads the catalog and the queryable raw
row within one deadline. Results describe that observation at the time of the
read. Retention can subsequently remove the evidence.

## 3. Two-Branch Stacked Development Workflow & Release Gate

Following the repository workflow anchored in [`knowledge/decisions/D-0001-development-workflow.md`](../knowledge/decisions/D-0001-development-workflow.md):

1. Agent authors develop linear atomic stacks via colocated Jujutsu (`jj`) bookmarks (`stack/<topic>/01-...`).
2. Every PR targeting `develop` runs CI (`development-checks.yml`), requires an updated Mermaid diagram for structural changes, and undergoes independent review (preferring an alternate model family or fresh standard/medium reasoning context) before linear rebase integration onto `develop`.
3. Only explicit promotion PRs from `develop` to `main` may update `main`, gated by the human-approved `main-release` environment in [`.github/workflows/main-human-gate.yml`](../.github/workflows/main-human-gate.yml).

```mermaid
sequenceDiagram
    participant Author as Author Agent (jj stack/*)
    participant CI as CI (development-checks.yml)
    participant Reviewer as Independent Reviewer (Standard/Medium)
    participant Develop as develop Branch
    participant Gate as main-release Human Gate
    participant Main as main Branch

    Author->>CI: Push stack/topic/01..N PR -> develop
    CI-->>Author: fmt, check, test, clippy, architecture, doc tests
    Author->>Reviewer: Request review (with Mermaid diff + fabric doctor proof)
    Reviewer-->>Develop: Approve & rebase-merge linear stack onto develop
    Develop->>Gate: Open promotion PR (develop -> main)
    Gate-->>Main: Human explicitly approves main-human-approval & merges
```

## 4. Production Systemd Stack & Coupled Custody Topology

On `fedora-blade`, `cargo xtask collection install-stack` provisions a coupled rootless user service stack confined inside `fabrico11y.slice` (`MemoryHigh=256M`, `MemoryMax=512M`, `CPUQuota=15%`, `IOWeight=30`, `TasksMax=64`) with explicit `TMPDIR=/data/tmp`, `KillMode=control-group`, and no rootless namespace mounts (`PrivateTmp` / `ProtectSystem=strict`) that fail under SELinux user managers.

```mermaid
flowchart LR
    subgraph Slice["fabrico11y.slice (MemoryHigh=256M, MemoryMax=512M, CPUQuota=15%, TasksMax=64)"]
        SRV["fabric-server.service<br/>(/data/fabrico11y/server/{canonical,projection})"]
        SPN["spindle.service<br/>($XDG_RUNTIME_DIR/spindle/spindle.sock)"]
        DMN["fabric-daemon.service<br/>(/data/fabrico11y/{state,olap})"]
    end

    TOK1["spindle-node.token (0600)"] -->|"authorizes origin"| SRV
    TOK1 -->|"authenticates forwarding (127.0.0.1:17440)"| SPN
    TOK2["fabric-reader.token (0600)"] -->|"authorizes canonical reads"| SRV
    SPN -->|"persistent origin -> canonical authority"| SRV
    SPN -->|"poll custody records"| DMN
    DMN -->|"persist spindle_watermark_id post-Parquet flush"| SPN
    CLI["fabric custody --json & fabric doctor --json"] -->|"inspect socket + HTTP frontier + slice self-health"| Slice
```

## Service self-telemetry custody

Service exporters submit observations through the local SDK into a dedicated
Spindle socket and custody directory. Each service instance owns a bounded SDK
queue. The telemetry Spindle owns a separate persistent origin and can forward
through the existing canonical protocol. Use separate local projection storage
when inspecting this origin alongside the workload Spindle origin.

```mermaid
flowchart LR
    SPN["Workload Spindle"] -->|"bounded SDK queue"| TELE["Telemetry Spindle socket and custody"]
    SRV["Fabric Server"] -->|"bounded SDK queue"| TELE
    TELE -->|"distinct origin and credential"| SRV
    TELE -->|"origin-bound reads"| LOCAL["Local telemetry projection"]
```

The telemetry Spindle runs with self-telemetry export disabled. Services suppress
per-record telemetry for activity on this lane and report queue and delivery
diagnostics locally.
