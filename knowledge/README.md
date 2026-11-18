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
| [D-0004 · Local Collection and One Canonical Authority](decisions/D-0004-local-collection-and-one-canonical-authority.md) | `agent` | `false` | Keep local custody and projection at the edge and use per-origin frontiers over loopback tunnels for fleet investigations |
| [D-0005 · Receipt-specific local projection evidence](decisions/D-0005-receipt-projection-evidence.md) | `agent` | `false` | Origin-bound committed frontier, guarded exact row evidence, and no backfill for legacy rows |
| [D-0006 · Product Toolchain and Verifier Cache Provenance](decisions/D-0006-verifier-cache-provenance.md) | `agent` | `false` | Exact product Rust pin, independent verifier compilers, and lock-scoped OPAM cache validation |
| [D-0007 · CI verification groups](decisions/D-0007-ci-verification-groups.md) | `agent` | `false` | Shared validation runs once while native evidence and release checks remain independent |
| [D-0008 · Reuse collected host samples for fleet evidence](decisions/D-0008-reuse-collected-host-samples-for-fleet-evidence.md) | `agent` | `false` | Bounded, opt-in forwarding of host snapshot summaries with distinct canonical node attribution |
| [D-0009 · Separate Spindle origin for service telemetry](decisions/D-0009-service-telemetry-origin.md) | `agent` | `false` | Give service telemetry separate local custody and a separate forwarding frontier using the existing SDK and protocol |
| [D-0010 · Canonical receipt projection readiness](decisions/D-0010-canonical-receipt-projection.md) | `agent` | `false` | Authority-qualified receipt identity and independently checked analytical evidence |
| [D-0176 · Qualify Fedora Tool Images Before Adoption](decisions/D-0176-ci-tool-images.md) | `agent` | `false` | Build and verify common and native tool images before measuring hosted jobs for digest adoption |
| [D-0181 · Versioned service settings](decisions/D-0181-versioned-service-settings.md) | `agent` | `false` | Bounded TOML and legacy JSON loading with explicit per-field collector precedence |
| [O-0001 · `fedora-blade` Resource Post-Mortem](observations/O-0001-fedora-blade-resource-postmortem.md) | `agent` | `false` | Empirical 13.2-day session failure modes (`/home` cache bloat, `/tmp` `tmpfs` -> `zram0` swap exhaustion, `user.slice` memory spikes, leaked transient units) mapped to `FabricO11y` collectors |
| [O-0002 · Converged Release Verification & Stack Install](observations/O-0002-converged-release-verification-and-install.md) | `agent` | `false` | Verus/Aeneas/SMT/Lean 4 proof scope, `arrow-ipc` fuzz preflight lesson, and coupled `fabrico11y.slice` systemd stack installer |
| [D-0011 · Elastic collector memory](decisions/D-0011-elastic-collector-memory.md) | `agent` | `false` | Proof-backed cgroup headroom and safe return of optional cache memory |
| [O-0180 · Release deployment and physical two-host qualification](observations/O-0180-two-host-tunnel-smoke.md) | `agent` | `false` | Exact local and canonical receipts, both services’ MELT delivery, and physical outage catch-up |
| [O-0182 · Effective resource settings under Linux pressure](observations/O-0182-resource-pressure.md) | `agent` | `false` | Cgroup working-set transitions and durable Spindle admission at a low descriptor limit |
| [R-0001 · Converged Rust Storage and Five Observability Pillars](research/R-0001-converged-rust-storage-and-five-pillars.md) | `agent` | `false` | Pure-Rust `redb` + 32 MiB Gorilla ring + ZSTD Parquet/DataFusion layout, `DDSketch`/`EwmaMadDetector`, and Freshness/Volume/Schema/Distribution/Lineage evaluators |
| [R-0002 · Formal Proofs and Fuzz Verification for Converged Telemetry Predicates](research/R-0002-converged-telemetry-formal-proofs-and-fuzzing.md) | `agent` | `false` | Verus + Charon/Aeneas Lean 4 contracts, Z3 QF_BV SMT checks, standalone Lean 4 Gorilla/rollup/watermark proofs, and 3 converged telemetry fuzz targets |
