# FabricO11y Development Rules

Read [README.md](README.md), [docs/architecture.md](docs/architecture.md), [docs/direction.md](docs/direction.md), and [knowledge/README.md](knowledge/README.md). Use Rust 1.100.0 (Edition 2024) for all product code and repository utilities under `xtask`, CI bootstrap scripts under `.github`, and agent configuration under `.codex`. Prefer pure-Rust crates without unnecessary C toolchain dependencies or external DB sidecars.

## Branches, Jujutsu Stacks, and Review Gates

- Use the existing user-installed `jj` from `PATH` with its colocated Git backend for snapshots, atomic commits, and stacked bookmarks (`stack/<topic>/01-...`). Configure `git config commit.template .github/.gitmessage`.
- **Single-branch model (`main`) + stacked `jj` PRs:**
  - `main`: the single protected integration and release branch, governed by [`.github/CODEOWNERS`](.github/CODEOWNERS) (`* @kmosoti`) and native GitHub branch protection / rulesets (required CI checks via `development-checks.yml`, `@kmosoti` CODEOWNER approval, linear rebase-only history, no direct push or force-push).
  - **`@kennmosoti-maker` collaborator workflow:** `@kennmosoti-maker` (Write / `push` access, non-admin) and local agent stacks develop atomic linear changes on stacked Jujutsu bookmarks (`stack/<topic>/01-...`) and open stacked PRs targeting `main`. Before landing onto `main`, PRs must pass CI (`development-checks.yml`), undergo independent review (prefer an alternative model family or a fresh independent standard/medium-reasoning reviewer context when agent-authored), and receive `@kmosoti` approval.
- **Hard publication constraints:** Never push directly to `main`, never force-push or merge into `main` bypassing PR/`CODEOWNERS` review, and never rewrite verified commits without updating verification receipts. Keep `archive/*` and `stack/*` bookmarks local unless explicitly asked to publish.

## Storage Routing, Zero `/tmp`, and Self-Preservation

- **Zero `/tmp` or `/dev/shm` usage:** Fedora mounts `/tmp` as RAM-backed `tmpfs`. Never write build caches, test directories, scratch files, or runtime telemetry state under `/tmp` or `/dev/shm`. Always route:
  - `CARGO_BUILD_TARGET_DIR=/data/cargo-target`
  - `TMPDIR=/data/tmp`
  - `FABRIC_DATA_DIR=/data/fabrico11y`
- **Zero UI scope:** Never reintroduce `fabric-view`, HTML/JS/CSS dashboards, or web UI servers. All operator and agent consumption goes through the deterministic `fabric` CLI (`--json`, `--arrow`, `--table`).
- **Slice confinement:** Daemon/collection tests and resident `fabric daemon` runs operate inside `fabrico11y.slice` (`MemoryHigh=256M`, `MemoryMax=512M`, `CPUQuota=15%`, `IOWeight=30`, `TasksMax=64`), keeping RSS `< 100 MiB` (`< 200 MiB` self-shedding ceiling) and open FDs `< 64`.
- **Transient systemd test units:** Whenever invoking `systemd-run` for tests or agent tools, always pass `--collect` (`CollectMode=inactive-or-failed`) so finished or failed units never leak in `systemd --user` RAM.

## Architecture & Sans-I/O Separation

- `admission-kernel`, `fabric-core`, and `fabric-graph` are strictly **Sans-I/O**: zero filesystem, socket, environment, or async runtime calls.
- `fabric-store`, `fabric-collect`, `fabric-query`, and `fabric-cli` isolate storage (`redb`, `HotMetricRing`, ZSTD `Parquet`), Linux sysfs/`/proc`/cgroup/`inotify` probes, DataFusion analytical planning, and CLI serialization.
- Preserved crates (`fabric`, `fabric-server`, `fabric-mcp`) and checked-in Weaver (`registry/`) / Verus-Charon-Aeneas proofs must keep passing every `cargo xtask` check. Update [`verification/architecture.toml`](verification/architecture.toml) whenever workspace crates or direct dependencies change.
- Update [`docs/architecture.md`](docs/architecture.md) (with current Mermaid diagrams) and [`knowledge/`](knowledge/README.md) (`added_by: agent`, `human_ratified: false` unless explicitly ratified by a human) whenever architecture, storage, or workflow contracts change.
- Keep `AGENTS.md` focused on operating rules, `docs/` focused on stable architecture and usage contracts, and `knowledge/` focused on dated decisions, empirical observations, and research.
