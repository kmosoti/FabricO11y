## Summary
- **Motivation / Why:**
- **Changes / What:**
- **Linked Issue(s):** Refs #

## Stack Position & Target Branch
- [ ] Standalone PR targeting `main`
- [ ] Part of a stacked PR chain targeting `main`:
  - Base branch for this PR: `main` (or `stack/<topic>/...`)
  - Stack order:
    1. #... (`stack/<topic>/01-...`)
    2. **-> This PR** (`stack/<topic>/02-...`)

## Architectural & Boundary Check
- [ ] Sans-I/O separation preserved (`admission-kernel`, `fabric-core`, and `fabric-graph` stay free of OS/filesystem/network I/O)
- [ ] Zero UI/frontend assets or web servers introduced (CLI `--json` / `--arrow` / `--table` only)
- [ ] Zero `/tmp` or `/dev/shm` writes at build or runtime (`CARGO_BUILD_TARGET_DIR=/data/cargo-target`, `TMPDIR=/data/tmp`, `FABRIC_DATA_DIR=/data/fabrico11y`)
- [ ] Self-preservation invariants respected (`fabrico11y.slice`, `rss_bytes < 100 MiB`, `open_fds < 64`, `tmp_files_created == 0`)

## Architecture for Human Review
- **Overall architecture:** [docs/architecture.md](docs/architecture.md)
- **Change diagram:** include a focused Mermaid diagram for changed crate boundaries, telemetry data flow, storage layout, or CI; for prose-only changes explain why no update is needed.
- **Ownership/trust boundaries and impact:**

## Verification Evidence
- **Commands executed & results:**
  - `TMPDIR=/data/tmp CARGO_BUILD_TARGET_DIR=/data/cargo-target cargo xtask architecture`
  - `TMPDIR=/data/tmp CARGO_BUILD_TARGET_DIR=/data/cargo-target cargo xtask check --tier medium`
  - `fabric doctor --json` (inside `fabrico11y.slice`)
- **CI/setup impact (when touched):** cache behavior, toolchain reproducibility, cold/warm timings.
- **Not run / untested platforms or environments:**
  - `...`

## Knowledge & Documentation
- **Knowledge records added/updated (`knowledge/`):** `none` (or list IDs with `added_by` and `human_ratified` status)
- **Stable docs updated (`docs/`):** `none` (or list files)

## Merge Readiness & Review Gate (`main` + `CODEOWNERS`)
- **Authoring agent / collaborator (`@kennmosoti-maker` or agent model):** `...`
- **Independent reviewer & `@kmosoti` CODEOWNER approval:** `...` (prefer alternative model family; fallback: fresh independent standard/medium reviewer)
- [ ] Reviewed by an independent standard/medium reviewer (when agent-authored) and approved by `@kmosoti` per [`.github/CODEOWNERS`](.github/CODEOWNERS) before rebase-merging bottom-up onto `main`
- [ ] Reviewer used standard/medium reasoning; max/ultra were not used
- [ ] Branch is rebased onto its target base with a clean linear history (no merge commits)
- [ ] All review conversations are resolved
