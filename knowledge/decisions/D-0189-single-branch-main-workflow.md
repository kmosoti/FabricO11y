# D-0189 · Single-Branch `main` Workflow with `CODEOWNERS` and Stacked Jujutsu PRs

## Decision

Retire the two-branch `develop` -> `main` promotion workflow and `.github/workflows/main-human-gate.yml` (`D-0001`) in favor of a single protected integration and release branch (`main`):

1. **Code Owner Gate (`.github/CODEOWNERS`):**
   - `* @kmosoti` requires maintainer review on every pull request targeting `main` via native GitHub branch protection / rulesets.
2. **Collaborator & Stacked Jujutsu PRs (`@kennmosoti-maker`):**
   - `@kennmosoti-maker` (Write / `push` collaborator, non-admin) and local authoring agents build linear atomic Jujutsu stacks (`stack/<topic>/01-...`, `02-...`) directly off `main` and submit stacked PRs targeting `main`.
   - Every PR runs `.github/workflows/development-checks.yml`, updates Mermaid architecture diagrams when boundaries change, and receives independent standard/medium-reasoning review (when agent-authored) before `@kmosoti` approves and rebase-merges bottom-up onto `main`.
3. **CI Trigger & Cache Alignment:**
   - `development-checks.yml`, `save-ci-cache`, `ci-images.yml`, and paired qualification plan ancestry checks target `main` exclusively so trusted cache saves and image publications occur on verified `main` pushes without duplicate branch runs.
4. **Preserved Host & Scope Invariants:**
   - Zero `/tmp` / `/dev/shm` usage (`CARGO_BUILD_TARGET_DIR=/data/cargo-target`, `TMPDIR=/data/tmp`, `FABRIC_DATA_DIR=/data/fabrico11y`), Zero-UI CLI scope, `fabrico11y.slice` confinement, and Sans-I/O crate isolation remain unchanged.

The historical product-wide UI prohibition above is superseded by
[D-0192](D-0192-optional-frontend.md). The other historical workflow and
operating constraints are governed by their current decisions and rules.

## Alternatives Considered

- **Keep `develop` + `main-human-gate.yml`:** Rejected because `main` and `develop` carried identical trees via rebase and the GitHub Environment promotion workflow added friction without extra safety over native `main` branch protection + `.github/CODEOWNERS` (`* @kmosoti`) + non-admin collaborator `push` permissions.

## Revisit When

- Multi-maintainer release trains require a dedicated release-candidate stabilization branch.

## Relations

- *supersedes*: [D-0001](D-0001-development-workflow.md)
- *aligned-with*: [D-0002](D-0002-cli-pivot-and-converged-telemetry-engine.md)
