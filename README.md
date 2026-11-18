# FabricO11y

`FabricO11y` is a **CLI-first, zero-UI Fedora/Linux host, agent, and OTLP telemetry sensory plane** backed by a pure-Rust converged storage and causal attribution engine. It tracks cgroup v2 resource pressure, PSI stalls, `zram0`/swap saturation, systemd transient unit hygiene (`--collect` leaks), high-churn filesystem paths (`/tmp`, `/data/tmp`, `~/.cache`, `target/`), optional GPU sysfs metrics, and OTLP/Spindle envelopes inside a single bounded daemon and deterministic `fabric` CLI.

## Converged Engine & CLI (`fabric`)

All data lives locally under `FABRIC_DATA_DIR` (default `/data/fabrico11y`) with **zero external database containers** and **zero `/tmp` or `/dev/shm` usage**:

- **`fabric-core` & `fabric-graph` (Sans-I/O):** Domain models, non-`/tmp` path enforcement, `DDSketch` quantiles, `EwmaMadDetector`, Five Data Observability Pillars (`Freshness`, `Volume`, `Schema`, `Distribution`, `Lineage`), causal graph BFS (`lineage`), and ranked resource attribution (`blame`).
- **`fabric-store`:** Embedded `redb` ACID catalog + adjacency graph (`catalog.redb`), `32 MiB` Gorilla delta/XOR `HotMetricRing`, ZSTD-compressed date-partitioned Arrow/Parquet OLAP store, and `4 GiB` `RetentionJanitor`.
- **`fabric-collect`:** Bounded `tokio` async collectors (`try_send` drop-with-counter) confined to `fabrico11y.slice` (`MemoryHigh=256M`, `MemoryMax=512M`, `CPUQuota=15%`, `IOWeight=30`, `TasksMax=64`) with a `200 MiB` RSS self-shedding watchdog.
- **`fabric-query` & `fabric-cli` (`fabric`):** DataFusion/Arrow unified queries and machine-readable CLI subcommands (`--json`, `--arrow`, `--table`).
- **Preserved Formal & Canonical Crates:** `admission-kernel` (Verus/Charon/Aeneas/Lean), `fabric` SDK + Spindle custody, `fabric-server` canonical receiver, and `fabric-mcp` stdio server.

```sh
# Deterministic host + agent + OTLP CLI
fabric snapshot --json
fabric query metrics --since 1h --json
fabric blame memory --json
fabric lineage /data/tmp/fabric-synth-load --direction both --json
fabric drift --pillar lineage --json
fabric custody --json
fabric doctor --json
fabric guide fleet --json
fabric settings
fabric daemon --interval-ms 2000

# Repository verification & coupled production user-stack install
TMPDIR=/data/tmp CARGO_BUILD_TARGET_DIR=/data/cargo-target cargo xtask architecture
TMPDIR=/data/tmp CARGO_BUILD_TARGET_DIR=/data/cargo-target cargo xtask check --tier medium
TMPDIR=/data/tmp CARGO_BUILD_TARGET_DIR=/data/cargo-target cargo xtask prove all
TMPDIR=/data/tmp CARGO_BUILD_TARGET_DIR=/data/cargo-target cargo xtask fuzz smoke /data/tmp/fabric-fuzz-smoke
TMPDIR=/data/tmp CARGO_BUILD_TARGET_DIR=/data/cargo-target cargo xtask collection install-stack --bin-dir /data/cargo-target/release --data-dir /data/fabrico11y --tmp-dir /data/tmp --listen 127.0.0.1:17440
```

See [architecture](docs/architecture.md), [product direction](docs/direction.md), [linked knowledge](knowledge/README.md), [contributing](CONTRIBUTING.md), [local collection setup](docs/collection.md), [canonical custody](docs/canonical-custody.md), [typed investigations](docs/investigation.md), the [offline guide](docs/guide.md), and [service settings](docs/settings.md).

Requires Jujutsu 0.46.0 with a Git backend and Rust 1.100.0, pinned in [`rust-toolchain.toml`](rust-toolchain.toml).
