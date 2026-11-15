# Linked Project Knowledge

This directory is a **versioned Markdown knowledge graph**, not a second set of product documentation. It retains why choices were made, what has actually been observed, conflicting evidence, and what would change a conclusion. It is readable by both humans and agents without an external graph database.

## Boundary with documentation

- [`docs/architecture.md`](../docs/architecture.md) and [`CONTRIBUTING.md`](../CONTRIBUTING.md): stable architecture contracts, CLI specifications, and development workflow instructions.
- `knowledge/`: dated observations, decisions, alternatives, and technical research notes with explicit authorship and ratification provenance.

## Provenance and human ratification (non-blocking for agents)

Every knowledge artifact distinguishes **who added it** from **whether a human explicitly ratified it**:

- `added_by`: `agent` | `human` | `mixed` — records whether the artifact was authored by an autonomous/assisted coding agent, a human contributor, or both.
- `human_ratified`: `true` | `false` — `true` **only** when a human user has explicitly ratified the artifact's claim or decision. When `true`, record `ratified_by` and `ratified_on` (`YYYY-MM-DD`).

### Zero-friction velocity rule

1. **No ratification gate for execution:** Agents create, link, update, and act on `knowledge/` artifacts immediately with `added_by: agent` and `human_ratified: false`.
2. **Asynchronous ratification:** Humans ratify records during review by setting `human_ratified: true`, `ratified_by`, and `ratified_on`, or by explicitly instructing an agent to ratify a named record.
3. **Authority semantics:**
   - `human_ratified: false` artifacts are active working knowledge that may be refined or superseded as new evidence arrives.
   - `human_ratified: true` artifacts carry explicit human confirmation and must not be silently contradicted by later agent decisions (`D-*`).

## Node types

| Directory | Prefix | Purpose |
| --- | --- | --- |
| `decisions/` | `D-` | Architectural choices, constraints, considered alternatives, and revisit triggers |
| `observations/` | `O-` | Dated reports of actual runtime state, incident post-mortems, and verification evidence |
| `research/` | `R-` | Technical evaluation of storage engines, sketch algorithms, and observability pillars |

## Connected nodes

| Node | Added by | Human ratified | Summary |
| --- | --- | --- | --- |
| [D-0001 · Two-Branch Stacked Development Workflow](decisions/D-0001-development-workflow.md) | `agent` | `false` | Two-branch (`develop` + `main-release` gated `main`) Jujutsu stacked workflow, linear rebase history, independent review, and PR Mermaid requirement |
| [D-0002 · CLI Pivot and Converged Telemetry Engine](decisions/D-0002-cli-pivot-and-converged-telemetry-engine.md) | `agent` | `false` | Zero-UI deterministic `fabric` CLI, pure-Rust `/data/fabrico11y` converged engine, Sans-I/O separation, and `fabrico11y.slice` self-preservation |
| [D-0003 · Spindle Custody Convergence and Sans-I/O Core Extraction](decisions/D-0003-spindle-custody-convergence-and-sans-io-core.md) | `agent` | `false` | Poll Spindle custody into `fabric-collect` with post-Parquet `spindle_watermark_id` durability, `fabric custody` inspection, and Sans-I/O extraction of `HotMetricRing` & `UnifiedQuerySpec` into `fabric-core` |
| [O-0001 · `fedora-blade` Resource Post-Mortem](observations/O-0001-fedora-blade-resource-postmortem.md) | `agent` | `false` | Empirical 13.2-day session failure modes (`/home` cache bloat, `/tmp` `tmpfs` -> `zram0` swap exhaustion, `user.slice` memory spikes, leaked transient units) mapped to `FabricO11y` collectors |
| [R-0001 · Converged Rust Storage and Five Observability Pillars](research/R-0001-converged-rust-storage-and-five-pillars.md) | `agent` | `false` | Pure-Rust `redb` + 32 MiB Gorilla ring + ZSTD Parquet/DataFusion layout, `DDSketch`/`EwmaMadDetector`, and Freshness/Volume/Schema/Distribution/Lineage evaluators |
| [R-0002 · Formal Proofs and Fuzz Verification for Converged Telemetry Predicates](research/R-0002-converged-telemetry-formal-proofs-and-fuzzing.md) | `agent` | `false` | Verus + Charon/Aeneas Lean 4 contracts, Z3 QF_BV SMT checks, standalone Lean 4 Gorilla/rollup/watermark proofs, and 3 converged telemetry fuzz targets |
