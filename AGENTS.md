# FabricO11y Development Rules

Read [README.md](README.md), [docs/architecture.md](docs/architecture.md), [docs/direction.md](docs/direction.md), and [knowledge/README.md](knowledge/README.md). Use Rust 1.100.0 (Edition 2024) for all product code and repository utilities under `xtask`, CI bootstrap scripts under `.github`, and agent configuration under `.codex`. Prefer pure-Rust crates without unnecessary C toolchain dependencies or external DB sidecars.

## Branches, Jujutsu Stacks, and Review Gates

- Use the existing user-installed `jj` from `PATH` with its colocated Git backend for snapshots, atomic commits, and stacked bookmarks (`stack/<topic>/01-...`). Configure `git config commit.template .github/.gitmessage`.
- **Two-branch model:**
  - `develop`: integration branch. Land linear `jj` stacks bottom-up after current-base CI and independent review. Use the non-admin collaborator account for agent PRs. Independent review covers protected policy and trust boundaries.
  - `main`: human release branch. Promote same-repository `develop` through a PR bound to the exact candidate and protected by `main-release`. The owner performs promotion, tagging and release approval. Follow the [promotion runbook](docs/release-promotion.md).
- **Hard publication constraints:** Never push directly to `main`, never merge into `main`, never approve `main-release`, and never rewrite verified commits without updating verification receipts. Keep `archive/*` and `stack/*` bookmarks local unless explicitly asked to publish.

## Storage Routing, Zero `/tmp`, and Self-Preservation

- **Zero `/tmp` or `/dev/shm` usage:** Fedora mounts `/tmp` as RAM-backed `tmpfs`. Never write build caches, test directories, scratch files, or runtime telemetry state under `/tmp` or `/dev/shm`. Always route:
  - `CARGO_BUILD_TARGET_DIR=/data/cargo-target`
  - `TMPDIR=/data/tmp`
  - `FABRIC_DATA_DIR=/data/fabrico11y`
- **Headless by default:** Keep the backend, CLI, MCP, and typed client free of renderer dependencies. Optional, separately packaged frontends use the shared typed boundary and require explicit installation.
- **Slice confinement:** Daemon/collection tests and resident `fabric daemon` runs operate inside `fabrico11y.slice` (`MemoryHigh=256M`, `MemoryMax=512M`, `CPUQuota=15%`, `IOWeight=30`, `TasksMax=64`), keeping RSS `< 100 MiB` (`< 200 MiB` self-shedding ceiling) and open FDs `< 64`.
- **Transient systemd test units:** Whenever invoking `systemd-run` for tests or agent tools, always pass `--collect` (`CollectMode=inactive-or-failed`) so finished or failed units never leak in `systemd --user` RAM.

## Architecture & Sans-I/O Separation

- `admission-kernel`, `fabric-core`, and `fabric-graph` are strictly **Sans-I/O**: zero filesystem, socket, environment, or async runtime calls.
- `fabric-store`, `fabric-collect`, `fabric-query`, and `fabric-cli` isolate storage (`redb`, `HotMetricRing`, ZSTD `Parquet`), Linux sysfs/`/proc`/cgroup/`inotify` probes, DataFusion analytical planning, and CLI serialization.
- Preserved crates (`fabric`, `fabric-server`, `fabric-mcp`) and checked-in Weaver (`registry/`) / Verus-Charon-Aeneas proofs must keep passing every `cargo xtask` check. Update [`verification/architecture.toml`](verification/architecture.toml) whenever workspace crates or direct dependencies change.
- Update [`docs/architecture.md`](docs/architecture.md) (with current Mermaid diagrams) and [`knowledge/`](knowledge/README.md) whenever architecture, storage, or workflow contracts change. Store knowledge record metadata in [`knowledge/records.toml`](knowledge/records.toml), keyed by stable record ID; use `added_by: agent` and `human_ratified: false` unless a human explicitly ratifies the record. Do not add YAML front matter or metadata comments to Markdown; preserve code examples and recognized formatter/spell-checker directives inside comments.
- Update [`xtask/documentation-inventory.toml`](xtask/documentation-inventory.toml) for every added, removed, or reclassified tracked Markdown file. Run `cargo xtask documentation` to validate inventory coverage, record metadata, internal links and references, Markdown fences, and metadata-comment policy.
- Keep `AGENTS.md` focused on operating rules, `docs/` focused on stable architecture and usage contracts, and `knowledge/` focused on dated decisions, empirical observations, and research.
