# Product Direction

`FabricO11y` turns Linux workstation, container, autonomous-agent, and OTLP forensics into a **continuous, deterministic CLI-first sensory plane**.

## Core Architectural Principles

1. **Zero UI Scope (`fabric` CLI Only):**
   - Operators and autonomous orchestrators (`agy`, `Metron`, CI harnesses) query telemetry strictly via the `fabric` CLI (`--json`, `--arrow`, `--table`) or the typed MCP server (`fabric-mcp`). Web UIs, browser bundles, and HTTP UI servers are out of scope.
2. **Human + Agent + Host Telemetry as First-Class Signals:**
   - Cgroup v2 slice accounting (`app.slice`, `agent.slice`, `session.slice`, `fabrico11y.slice`, `system.slice`), kernel PSI (`/proc/pressure/*`), `/sys/block/zram0/mm_stat`, `tmpfs` and disk filesystem churn (`/tmp`, `/data/tmp`, `~/.cache`, `target/`), unlinked open descriptors, and systemd transient unit `--collect` hygiene share the same causal graph (`fabric-graph`) and storage plane (`fabric-store`) as application traces, metrics, and logs.
3. **Zero-Daemon Converged Pure-Rust Storage (`/data/fabrico11y`):**
   - Workstation and node observability must not require heavyweight external containers (GreptimeDB, ClickHouse, Postgres). One binary embeds `redb` OLTP/graph state, a bounded `32 MiB` Gorilla RAM metric ring, and ZSTD-compressed Arrow/Parquet partitions queried through memory-capped DataFusion (`64 MiB` `GreedyMemoryPool`).
   - External federated custody (`fabric-server` + optional engine qualification) remains preserved for multi-node canonical aggregation.
4. **Hard Self-Preservation Invariants:**
   - Strictly forbid `/tmp` and `/dev/shm` writes across all crates and scripts.
   - Bound collection queues via non-blocking `try_send` drop-with-counter backpressure, confine the daemon inside `fabrico11y.slice` (`MemoryHigh=256M`, `MemoryMax=512M`, `CPUQuota=15%`, `IOWeight=30`, `TasksMax=64`), shed load above `200 MiB` RSS, and prove compliance at runtime with `fabric doctor --json`.
5. **Verified Semantic Kernels & Explicit Epistemic Boundaries:**
   - Distinguish raw observed facts, `DDSketch` / `EwmaMadDetector` approximations, Five Data Observability Pillar evaluations (`Freshness`, `Volume`, `Schema`, `Distribution`, `Lineage`), and causal graph lineage edges. Keep formal proofs (`admission-kernel` via Verus/Charon/Aeneas/Lean) and Sans-I/O cores strictly isolated from OS I/O crates.
