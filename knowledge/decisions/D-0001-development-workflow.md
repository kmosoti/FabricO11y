---
id: D-0001
title: Two-Branch Stacked Development Workflow and Human Gate on main
type: decision
status: superseded
date: 2026-11-14
added_by: agent
human_ratified: false
superseded_by: D-0189
related:
  - D-0002
  - O-0001
---

# D-0001 · Two-Branch Stacked Development Workflow and Human Gate on `main`

## Decision

Adopt the two-branch repository governance model established in `rust-ui-template`:

1. **`develop` (Agent Integration Branch):**
   - Day-to-day feature, fix, proof, and refactoring work lands via atomic Jujutsu (`jj`) commits and stacked local bookmarks (`stack/<topic>/01-...`, `02-...`).
   - Before integrating onto `develop`, changes must pass CI (`development-checks.yml`) and receive an independent review (preferring an alternative model family or fresh standard/medium-reasoning reviewer context) using linear rebase-only history.
2. **`main` (Human-Gated Release Branch):**
   - Receives changes **only** via promotion PRs from `develop`.
   - Protected by [`.github/workflows/main-human-gate.yml`](../../.github/workflows/main-human-gate.yml) (`main-human-approval` job bound to GitHub Environment `main-release`).
   - Autonomous agents must never merge into `main`, push directly to `main`, or approve `main-release`.
3. **Commit & PR Discipline:**
   - Conventional commit messages follow [`.github/.gitmessage`](../../.github/.gitmessage).
   - Pull requests use [`.github/pull_request_template.md`](../../.github/pull_request_template.md) and must include a focused Mermaid diagram alongside [docs/architecture.md](../../docs/architecture.md) changes and verification evidence.

## Constraints

- Colocated Jujutsu (`jj`) over the Git backend; bookmarks `develop`, `main`, and `stack/*` remain explicit.
- Zero `/tmp` usage in CI bootstrap or local build/test recipes (`TMPDIR=/var/tmp` in CI containers; `TMPDIR=/data/tmp` and `CARGO_BUILD_TARGET_DIR=/data/cargo-target` locally on developer workstations).

## Alternatives Considered

- **Single-branch `main`-only integration:** Rejected because autonomous agent iteration and human release sign-off need separate blast radii and asynchronous promotion.
- **Merge commits on `develop` / `main`:** Rejected in favor of linear rebase history for clean `jj` stack restacking and bisectability.

## Revisit When

- Multi-repository release automation or automated signed tags require an additional release-candidate branch.

## Relations

- *informed-by*: [O-0001](../observations/O-0001-workstation-resource-postmortem.md)
- *enables*: [D-0002](D-0002-cli-pivot-and-converged-telemetry-engine.md)
