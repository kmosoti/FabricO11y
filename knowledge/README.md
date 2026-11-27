# Linked Project Knowledge

This directory is a **versioned Markdown knowledge graph**, not a second set of product documentation. It retains why choices were made, what has actually been observed, conflicting evidence, and what would change a conclusion. It is readable by both humans and agents without an external graph database.

## Boundary with documentation

- [`docs/architecture.md`](../docs/architecture.md) and [`CONTRIBUTING.md`](../CONTRIBUTING.md): stable architecture contracts, CLI specifications, and development workflow instructions.
- `knowledge/`: dated observations, decisions, alternatives, and technical research notes with explicit authorship and ratification provenance.

## Provenance and human ratification (non-blocking for agents)

Every knowledge artifact distinguishes **who added it** from **whether a human explicitly ratified it**:

- `added_by`: `agent` | `human` | `mixed` — records whether the artifact was authored by an autonomous/assisted coding agent, a human contributor, or both.
- `human_ratified`: `true` | `false` — `true` **only** when a human user has explicitly ratified the artifact's claim or decision. When `true`, record `ratified_by` and `ratified_on` (`YYYY-MM-DD`).

### Adding or changing metadata

Add one `[[records]]` entry in `records.toml` for each record, preserving the
stable ID, exact provenance, ratification state, and relations. Keep absent
fields absent; do not infer a title, status, or ratification. The `source_path`
maps the ID to its Markdown body. A record whose ID was historically implicit
in its filename retains that ID as its structured key. `migration.migrated_ids`
is the immutable cohort copied from the recorded source commit; add later
TOML-native records without changing that cohort or its count.

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
| [D-0001 · Two-Branch Stacked Development Workflow](decisions/D-0001-development-workflow.md) | `agent` | `false` | Superseded by D-0189; legacy two-branch (`develop` + `main-release` gated `main`) Jujutsu stacked workflow |
| [D-0189 · Single-Branch `main` Workflow with `CODEOWNERS`](decisions/D-0189-single-branch-main-workflow.md) | `agent` | `false` | Historical, superseded by D-0191. Single-branch `main` + stacked `jj` PRs, `.github/CODEOWNERS` (`* @kmosoti`), and `@kennmosoti-maker` collaborator workflow |
| [D-0190 · Zero-UI Typed Consumer Crate (`fabric-client`)](decisions/D-0190-zero-ui-fabric-client.md) | `agent` | `false` | Zero-UI `fabric.cli.v1` schema crate (`fabric-client`), `FABRIC_CREDENTIAL_FILE`-only `FabricQueryClient`, env stripping, redaction, and `kill_on_drop` |
| [D-0191 · Verified develop integration and human release promotion](decisions/D-0191-verified-develop-integration.md) | `agent` | `false` | Current-base integration, exact promotion and final artifact qualification |
| [D-0194 · Causal contribution projection](decisions/D-0194-causal-contributions.md) | `agent` | `false` | Bounded runtime-neutral contribution and proposal heads |
| [D-0207 · Prior-failure claims](decisions/D-0207-prior-failure-claims.md) | `agent` | `false` | Frozen-basis observation references without verification claims |
| [D-0206 · Selected dependency differential](decisions/D-0206-selected-differential.md) | `agent` | `false` | Compare caller-bound frozen dependencies without inferred chronology |
| [D-0205 · Selected-head reconciliation](decisions/D-0205-selected-head-reconciliation.md) | `agent` | `false` | Explicit bounded reconciliation of complete selected ancestry |
| [D-0204 · Frozen citation context](decisions/D-0204-frozen-context.md) | `agent` | `false` | Reverify bounded supplied passages within selected frozen evidence |
| [D-0203 · Bounded lexical rebuild](decisions/D-0203-bounded-lexical.md) | `agent` | `false` | Deterministic within-revision token adjacency with explicit limits |
| [D-0202 · Portable corpus views](decisions/D-0202-portable-corpus.md) | `agent` | `false` | Bounded deterministic view transfer and availability claims |
| [D-0201 · Explicit corpus as-of views](decisions/D-0201-corpus-as-of.md) | `agent` | `false` | Bounded frozen membership and conjunctive support claims |
| [D-0348 · Receipt-bound reported attempts](decisions/D-0348-receipt-bound-attempts.md) | `agent` | `false` | Strict authenticated attempt readback and its limits |
| [D-0239 · Protected application capture](decisions/D-0239-protected-capture.md) | `agent` | `false` | Policy-bound protected outboxes and sanitized application artifacts |
| [D-0200 · Frozen experiment snapshots](decisions/D-0200-frozen-snapshots.md) | `agent` | `false` | Fixed roster, explicit boundaries and missingness-aware comparisons |
| [D-0270 · Conditional canonical plan acceptance](decisions/D-0270-conditional-plan-acceptance.md) | `agent` | `false` | Authenticated single-owner scope and atomic decision retry |
| [D-0223 · Live cursor cadence](decisions/D-0223-live-cursor-cadence.md) | `agent` | `false` | Bounded opt-in Spindle polling |
| [D-0199 · Canonical query operation negotiation](decisions/D-0199-query-negotiation.md) | `agent` | `false` | Authenticated opt-in per-operation version gate |
| [D-0304 · Portable query operation contracts](decisions/D-0304-portable-query-contracts.md) | `agent` | `false` | Shared pure query DTOs and native compatibility |
| [D-0198 · Operation dispatch profiles](decisions/D-0198-dispatch-profiles.md) | `agent` | `false` | Scoped dependency profiles and matched dispatch measurement |
| [D-0197 · Typed operation contracts](decisions/D-0197-typed-operation-contracts.md) | `agent` | `false` | Versioned typed registry and payload-free outcome categories |
| [D-0195 · Runtime-neutral operation lifecycles](decisions/D-0195-operation-lifecycle.md) | `agent` | `false` | Bounded lifecycle transitions and adapter-established trust context |
| [D-0196 · Bounded application result identity](decisions/D-0196-application-results.md) | `agent` | `false` | Scoped result identity and separate source and canonical application digests |
| [D-0193 · Research corpus revision identity](decisions/D-0193-corpus-revisions.md) | `agent` | `false` | Transport-free immutable revision and claim boundaries |
| [D-0192 · Headless defaults and an optional frontend](decisions/D-0192-optional-frontend.md) | `agent` | `false` | Supersedes D-0002's product-wide UI prohibition and retains the D-0190 client boundary |
| [D-0236 · Bounded run and attempt manifests](decisions/D-0236-bounded-run-attempt-manifests.md) | `agent` | `false` | Versioned Sans-I/O shared run and attempt context, typed digest roles, explicit disclosure and clock provenance, and pre-decode byte bounds |
| [D-0251 · Bounded MCP status scheduling](decisions/D-0251-bounded-mcp-status.md) | `agent` | `false` | Queues short status calls with a shared queue and transport deadline, and reserves independent cancellation capacity |
| [D-0399 · Fixed-barrier exact receipt batches](decisions/D-0399-bounded-receipt-batch.md) | `agent` | `false` | Bounded scoped receipts with one manifest inspection and independent outcomes |
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
| [D-0185 · Durable batch reports use the existing Spindle generation](decisions/D-0185-batch-outbox.md) | `agent` | `false` | Exact replay from a bounded client outbox with separate run and transport identity |
| [D-0288 · Local Unix HTTP adapter](decisions/D-0288-local-http.md) | `agent` | `false` | Typed, bounded Reader and Writer access on a private Unix socket |
| [O-0001 · `workstation-01` Resource Post-Mortem](observations/O-0001-workstation-resource-postmortem.md) | `agent` | `false` | Empirical 13.2-day session failure modes (`/home` cache bloat, `/tmp` `tmpfs` -> `zram0` swap exhaustion, `user.slice` memory spikes, leaked transient units) mapped to `FabricO11y` collectors |
| [O-0002 · Converged Release Verification & Stack Install](observations/O-0002-converged-release-verification-and-install.md) | `agent` | `false` | Verus/Aeneas/SMT/Lean 4 proof scope, `arrow-ipc` fuzz preflight lesson, and coupled `fabrico11y.slice` systemd stack installer |
| [D-0011 · Elastic collector memory](decisions/D-0011-elastic-collector-memory.md) | `agent` | `false` | Proof-backed cgroup headroom and safe return of optional cache memory |
| [O-0176 · Hosted CI tool image qualification](observations/O-0176-hosted-ci-image-qualification.md) | `agent` | `false` | Runner matching, Cargo cache freshness, and isolated hosted cache restores |
| [O-0180 · Release deployment and physical two-host qualification](observations/O-0180-two-host-tunnel-smoke.md) | `agent` | `false` | Exact local and canonical receipts, both services’ MELT delivery, and physical outage catch-up |
| [O-0182 · Effective resource settings under Linux pressure](observations/O-0182-resource-pressure.md) | `agent` | `false` | Cgroup working-set transitions, low-descriptor admission, and receiver custody replay after pressure and restart |
| [O-0187 · Common release compiler seed](observations/O-0187-common-compiler-seed.md) | `agent` | `false` | Exact release build timing, compiler-cache input scope, and writable overlay |
| [O-0188 · Offline CI qualification and retained hosted evidence](observations/O-0188-ci-qualification-simulation.md) | `agent` | `false` | Simulation boundary, incomplete native measurements, and required live evidence |
| [O-0294 · Product verifier and dependency-policy evidence](observations/O-0294-product-verifier-and-dependency-policy.md) | `agent` | `false` | Exact product and proof tool identities, fixture checks, lock resolution scope, and missing dependency-policy gates for #294 |
| [O-0300 · Existing proof inventory and trust boundaries](observations/O-0300-proof-inventory-and-trust-boundaries.md) | `agent` | `false` | Current proof obligations and pins, extraction/model boundaries, false-checker controls, hosted baseline result, and runtime trigger evidence still needed by #235 |
| [R-0001 · Converged Rust Storage and Five Observability Pillars](research/R-0001-converged-rust-storage-and-five-pillars.md) | `agent` | `false` | Pure-Rust `redb` + 32 MiB Gorilla ring + ZSTD Parquet/DataFusion layout, `DDSketch`/`EwmaMadDetector`, and Freshness/Volume/Schema/Distribution/Lineage evaluators |
| [R-0002 · Formal Proofs and Fuzz Verification for Converged Telemetry Predicates](research/R-0002-converged-telemetry-formal-proofs-and-fuzzing.md) | `agent` | `false` | Verus + Charon/Aeneas Lean 4 contracts, Z3 QF_BV SMT checks, standalone Lean 4 Gorilla/rollup/watermark proofs, and 3 converged telemetry fuzz targets |
