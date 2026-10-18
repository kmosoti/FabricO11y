# Recursive hexagonal execution and bounded query refinement

## Status, ownership and release boundary

**Status: proposed milestone specification; no implementation or acceptance is claimed.**

Target release metadata: `v0.1.0-alpha.2`. Planning date: 2026-10-18.
Planning baseline: `f001175ff4006e2f95294458ce36f4f089d3be7d`.
Tracking epic: [recursive execution and query refinement](https://github.com/kmosoti/FabricO11y/issues/88).
Capability branch: `milestone/recursive-hexagonal-execution`.
Machine-readable backlog: [plan.toml](recursive-hexagonal-execution/plan.toml).
Mission authoring scaffold: [mission-template.toml](recursive-hexagonal-execution/mission-template.toml).

Creating this plan does not start implementation, publish a release, change package versions, or change the current candidate. Product work is held until the ongoing first-alpha release completes and its actual artifacts are available, unless the owner explicitly authorizes isolated preparatory work. Such authorization must preserve the current candidate, accepted gates, hardware budgets and evidence ownership. Branch isolation alone is not resource isolation.

The [product contract](../PRODUCT-CONTRACT.md), accepted decisions, [existing layer map](../architecture/layers.json), [agent contract](../../AGENTS.md), and [release-readiness plan](release-readiness.md) remain authoritative. Proposed changes to those policies require a separate reasoned trust-boundary commit and acceptance before dependent implementation. This document does not silently supersede them.

## Objective and finite scope

Use the owner's Recursive Hexagonal Rust paradigm to implement a measured, compatible refinement of FabricO11y:

1. Minimal independently enforced mission execution: locked contracts, bounded isolated attempts, protected verification, durable completion records and per-mission release-note fragments.
2. One query-cost improvement, initially late materialization through a bounded candidate-selection primitive and statically composed query path.
3. One resource-control improvement, selected from lifecycle accounting/admission or cooperative query cancellation using the baseline evidence.
4. Candidate-specific integration, an upgrade from the actual published predecessor, and the applicable exact-package acceptance gates.

Success means less avoidable work or more predictable resource ownership under the registered workload, while preserving complete observable behavior. A structural refactor alone is not the claimed performance outcome. If a selected candidate fails its registered criteria, retain the reference path and failure record; do not weaken the criterion or quietly substitute a different project.

No GPU backend, new general-purpose query engine, distributed storage, new wire format, weaker durability, blanket crate reorganization, or general agent-platform implementation is included. No 1,000-identity qualification claim is imported into the bounded 10/100-identity release scope. At most two experimental implementations may be active at once; measured heavy work remains serialized under admitted resource limits.

## Source and explicit adaptations

The source is the owner-uploaded **Recursive Hexagonal Rust Agent Blueprint.md**, headed *Autonomous Agent Architecture & Execution Blueprint (`v0.1.0-alpha.2`)*. Original SHA-256: `2e1a22f80a36f11147d77c75b6a09470370e40f0a0d77e71e965f9c6ef1ddcd8`. Parts 2-5 supply the architecture, three mission types, mission template and five-stage oracle. Part 1 supplies the generator/verifier and isolated-attempt framing. The original remains an input proposal, not an adopted scientific result or compiled implementation.

The following adaptations are explicit design proposals for M01, not silent corrections to the source:

| Source mechanism | Milestone treatment |
| --- | --- |
| Level 0 primitives, required/provided ports, Level N composites | Retain. Record composition depth separately from semantic/effect layer. Map the workspace, but migrate only the selected paths. |
| Every component moved to `crates/ports`, `crates/primitives`, `crates/composites` | Defer physical relocation. Existing crates and modules remain unless a concrete dependency boundary justifies a small move. |
| Static composition implies inlined, zero-cost machine code | Static hot-path wiring is the default design. Inlining, speed, allocation cost and code size remain measured properties, not automatic guarantees. |
| Transparent newtype adapters | Use for a layout-compatible single-field bridge when appropriate. Representation does not prove adapter logic has zero cost. |
| No ad-hoc collections in composites | Reuse a verified primitive when it exists and fits the contract. Allow explicitly budgeted scratch/state collections; do not create a trait around every language operation. |
| Every port is independent of all other workspace code | Preserve the current allowance for `fabric-ports` to use core domain types. Any recursive extension must respect, or explicitly amend, the existing graph. |
| Strict error algebra and no panic conveniences in libraries | Require explicit deterministic error variants on the scoped primitive/composite production paths. Do not force equality on unsuitable payload types or conceal adapter I/O causes. Existing unrelated library code is not a blanket cleanup task. |
| All tests, contracts and manifests are immutable | Lock them per admitted implementation attempt. Contract/verifier/test changes occur in separately authorized missions and commits before implementation. Independent baseline oracles cannot be edited by the worker being judged. |
| `git reset --hard` for retries | Prefer a fresh disposable worktree from the pinned base. Reset is permitted only in a verified harness-owned disposable worktree after failure evidence and process cleanup are secured. Never reset the owner's checkout or published branches. |
| Three failed attempts justified by empirical scaling laws | Retain three consecutive assertion failures as the project circuit breaker. Do not claim a universal scientific optimum. Infrastructure retries have their own finite allowance. |
| `SubmitMission` is available | It names a required future harness boundary. Bind it to a real adapter in M03; no such callable tool is established by this plan. |
| Miri and deterministic counters prove soundness/performance | Keep these checks within their tested scope. Require native lifecycle and workload measurements where appropriate. Callgrind cache statistics are simulated, not hardware measurements. |
| Illustrative `arch-verifier` sample is production-ready | Do not copy it unchanged. Extend current tooling with resolved dependencies, fail-closed classification, syntax-aware checks and independently injected violations. |
| Part 1 success percentages and model/benchmark claims | Not adopted as evidence. No referenced experimental data in the attachment establishes them for FabricO11y. |

The source's graph router, CSR, Radix Heap and bitmap examples illustrate composition; they are not new telemetry requirements. A generated system prompt is a worker briefing, not an enforcement boundary.

## Computational signature and invariants

FabricO11y is an append-heavy, selectively queried, durability-constrained system with mixed interactive and background work. Optimize data movement, decoding, materialization and resource lifetimes before introducing more parallel execution.

The following remain invariant across reference and candidate:

- Batch and Strand identities, exact retry bytes, sequence and deduplication semantics, custody transfer, sync ordering, and known-failure versus interrupted-append handling.
- Query values, integer/rate semantics where exercised, total-order tie handling, snapshot pagination and credential binding.
- Freshness, gaps, unavailable-source reporting, completeness, retained boundaries and documented error/refusal behavior. Equal rows alone are insufficient.
- Publication before checkpoint/reclaim; retention must not destroy unacknowledged custody to satisfy a storage budget.
- Existing access roles, authorization, native service ownership, shutdown, TLS and sandbox requirements.
- Resource containment, failure preservation and independent delivery/query/rate oracle ownership.

The actual console Walk/scoped query path is the baseline for console improvements. Existing general Scan and candidate paths remain distinct. Rate-query expansion is deferred, not added to the console as part of this milestone.

## Recursive hexagonal implementation paradigm

### Two architectural dimensions

**Dimension A: semantic/effect layer.** Keep core, ports, app, adapter support, adapters, composition roots, verification and tooling from the existing layer map.

**Dimension B: composition depth.** Level 0 is a leaf algorithm or data structure with an explicit capability contract. Level N combines lower-depth capabilities without importing their implementation details. Depth is not a claim about CPU threads, runtime recursion or process placement.

A component must satisfy both dimensions. A lower composition depth never authorizes a forbidden effect or dependency. Production cores do not read time, files, environment or process state. Clock observations, cancellation signals, resource measurements and I/O enter through explicit data or appropriate effect ports.

Provided ports describe a capability an implementation offers. Required ports describe the minimum capability a composite needs. Prefer associated types, borrowed inputs, explicit scratch ownership, bounded outputs and deterministic errors. Use static generic wiring in the registered hot paths. An exception for dynamic dispatch needs a named location, rationale, budget and comparison; do not ban trait objects in unrelated cold adapters merely by association.

### Initial mapping, not a new workspace layout

| Existing area | Proposed use |
| --- | --- |
| `fabric-core` and core query modules | Preserve pure semantic kernels. Candidate pure key/budget state belongs here only when its responsibility and existing purity policy fit. |
| `fabric-ports` / `fabric-app` | Reuse effect boundaries and orchestration. A new algorithmic port requires a meaningful variation or testing boundary and separate architecture acceptance where needed. |
| `fabric-server/src/query/selection.rs` | Investigate the existing bounded selection implementation before creating another heap abstraction. |
| `fabric-server/src/query.rs`, `tail.rs`, segment readers | Compose source traversal, candidate selection and delayed row materialization while keeping evidence evaluation sound. |
| `fabric-server/src/http.rs`, `console.rs`, lifecycle owners | Wire cancellation or reservations through actual worker/response lifetimes without changing the public release contract. |
| `xtask`, existing verification and resource-launcher tooling | Host mission admission and verification adapters; do not put filesystem/process effects into the pure semantic core. |

Potential Level 0 capabilities are full-key comparison, bounded candidate selection, checked byte-accounting transitions and cancellation/progress state. A Level 1 selector consumes candidates, tracks a bounded threshold and preserves ties. A higher query composite combines traversal, selection, materialization and evidence. This is an architecture hypothesis to validate against the existing code, not permission to create all those types or ports in advance.

### Primitive and composite rules

Each primitive records input domain, pre/postconditions, invariants, ownership, failure algebra, capacity limits, complexity assumptions and measured operating regimes. Retain a simple safe reference implementation for differential checks.

Use `no_std` and explicit `alloc` policy where the component responsibility supports them. `no_std` does not mean allocation-free. Pure composites forbid unsafe code. Any newly proposed unsafe primitive needs separate admission, a local `SAFETY` argument, appropriate Miri/property/fuzz/model checks and a portable safe fallback where applicable. No new unsafe code is necessary merely to satisfy this milestone's name.

Register zero allocation only for a specified operation after a specified preparation phase. Charge preparation, scratch, retained data and cleanup separately. Do not assert that the entire service is zero-allocation. Measure monomorphization's build-time/code-size effects alongside execution cost. Do not enable host-specific instructions in distributed packages without a compatibility strategy.

## Mission protocol and trust boundaries

### Mission taxonomy

Retain exactly the blueprint's three implementation mission types:

- `PORT_CONTRACT`: specify/stabilize signatures, errors, invariants and prospective checks.
- `PRIMITIVE_OPT`: implement or optimize a leaf against the locked contract.
- `COMPOSITE_WIRE`: wire verified lower-depth capabilities and adapters into a higher-level behavior.

Release admission, workload registration and package acceptance are **gate activities**, not additional mission types and not disguised primitives. Harness I/O remains in tooling/adapters even when its pure state machine uses the same primitive/composite method.

### Admission and state transitions

Normalized lifecycle:

`Backlog -> Blocked | Worktree_Init -> In_Progress -> 5_Stage_Oracle -> Closed | Rollback_Retry | Escalated_HITL`

`Blocked -> Worktree_Init` requires the missing admission evidence. `Rollback_Retry -> Worktree_Init` requires preserved failure evidence, confirmed cleanup and remaining attempts. An interrupted verifier produces no completion receipt; resume starts from an independently validated checkpoint. `Closed` requires an accepted result at the exact verified postimage, not just a worker patch. A changed postimage must be verified again.

Admission binds the mission ID and schema version, actual base commit/tree, full allowlisted writes, expanded protected-file inventory and hashes, accepted dependencies and interface revisions, exact commands/toolchains/features, budgets, expected artifacts and verifier identity. Reject duplicate IDs, cycles, missing dependencies, unknown enum values, inconsistent paths, unsupported targets, placeholders and unbound budgets. All gates are explicit; missing fields do not become permissive defaults.

The checked-in `plan.toml` is a planning backlog, not runtime state. The authoring scaffold has `execution_admitted = false`. The trusted coordinator generates an admitted manifest and owns attempt state outside worker-writable paths. No `Closed` string edited in a manifest or GitHub checkbox grants acceptance.

### Isolated attempts and protected oracles

Use an owned worktree or copy-on-write workspace with a pinned base and an exclusive write lease. Overlapping mutable scopes cannot run together. Protected inputs, trusted command configuration, oracle code and completion records live outside the worker's write authority. Read-only mounts and OS permission boundaries enforce prevention; hashes bind identity and detect drift but do not alone prevent tampering.

A Git worktree is not a sandbox. A worker must not gain access to the owner's checkout, Git common-directory references, credentials, current candidate data, other missions' scratch or trusted verifier state. Restrict network and side effects to the declared fixture/task scope. The trusted coordinator performs repository publication. Verification executes candidate code inside appropriate containment; the candidate cannot write a green receipt or replace the executable that judges it.

Use the existing resource launcher and host-specific approved storage root. Verify containment before admitting work, preserve logs/receipts before cleanup, join/stop all owned processes, and account for build and verification resources as well as runtime resources. Do not inherit the laptop's budget on a smaller VPS. This plan grants no permission for privileged installation, destructive fault experiments or release publication.

### Failure distillation and bounded retries

On an assertion failure, preserve the diff, exact commands/exits, relevant logs, input identities, resource/cleanup observations and the minimal reproducer. Produce a **Negative Constraint Record** with the attempted mechanism, observed failed assertion, artifact references, ruled-out approach, and a root-cause hypothesis labeled as such unless demonstrated. A summary is not a substitute for raw evidence.

After confirmed cleanup, the next worker receives the mission, clean base and negative constraints rather than the entire failed conversation. Three consecutive assertion-failed attempts escalate to `Escalated_HITL`. Counter state survives worker restart. Contract insufficiency, scope tampering or uncertain cleanup escalates immediately. Infrastructure failures remain visible and use a separately capped retry budget; no infinite loop is hidden outside the assertion counter.

The worker submits a candidate to the future `SubmitMission` boundary. The trusted verifier/coordinator owns closure and writes the atomic `.changelog/alpha.2/<mission-id>.md` fragment after acceptance. Fragments are release metadata, not architecture-document filenames; the aggregator sorts stable mission IDs and refuses duplicates or unverified fragments. A fragment is not itself an evidence receipt.

## Five-stage external verification oracle

| Stage | Required claim | Mechanism and refusal behavior |
| --- | --- | --- |
| 1. Type, lint and attributes | The exact candidate compiles and follows the admitted policy. | Locked compiler/features, scoped build/tests and Clippy, changed-scope attribute checks. Log real command and exit. |
| 2. Architectural dependencies | Both existing layers and scoped recursive composition rules hold. | Extend `cargo xtask check-layers` and `check-core-purity`; resolve package identity and all admitted dependency kinds/targets. Reject unknown classification and demonstrate representative violations are caught. |
| 3. Functional and property invariants | The primitive/composite preserves the contract. | Independent conformance/oracle corpus, properties, metamorphic tests and selected negative controls. For queries compare complete page chains and evidence, not only rows. |
| 4. UB, aliasing and lifecycle | Applicable low-level and native interaction obligations are exercised. | Pin Miri for supported pure kernels; preserve native thread/process/cancellation and relevant fault tests. Required unsupported checks block acceptance or require an explicit prospective alternative, never a silent skip. |
| 5. Allocation, instructions and service cost | The registered improvement and regression budgets hold. | Instrumented allocation/instruction checks plus repeated native full-workflow measurements, code-size/build-cost observation and integrated resource gates. Metrics remain correctly labeled. |

Stages are a grouping of existing and mission-specific evidence, not five invented commands. Bind every command and its expected output before admission. Never assume `arch-verifier`, `SubmitMission`, or a `--check-budget` flag exists because it appears in the uploaded example.

The new architecture checker must cover renamed/path/patched packages, normal/build/dev/optional/target-specific edges and the feature/target matrix being claimed. Preserve existing documented exceptions. Match resolved package IDs rather than only dependency names. Fail closed on missing or malformed metadata for admitted components; map legacy components explicitly instead of silently skipping them. Use syntax-aware checks and compiler enforcement where available. Source substring matches do not prove active attributes. Crate graphs do not by themselves establish every module, macro or runtime interaction; state those limits and test touched module boundaries.

Bootstrap trust is independent: accepted contracts and negative controls are frozen outside the implementation worker's scope before M02 or M03 can certify missions. The changed verifier does not establish its own acceptance by approving its own patch. Extend existing trusted CI/receipt boundaries and require independent representative mutation controls.

## Registered measurement and promotion rule

G1 defines the baseline before candidate optimization, then freezes a protocol and numerical budgets before M05/M08 implementation. The source's illustrative 2% instruction ceiling is not automatically a universal gate. Record the improvement threshold, supported noise envelope, repetition/ordering rules, warm/cold-cache handling, input sizes/selectivity, resource ceilings and rejection criteria prospectively.

Required workload slices include selective and broad reads, empty results, wide bodies/attributes, complete pagination, journal and Segment sources, reconnect bursts, concurrent ingestion/query/sealing, slow/canceled clients, and selected existing corruption/retention/publication controls. The console's real Walk path and a safe reference path are distinct baselines. Use identical semantics and workload offers.

Record offered/admitted/committed work, backlog, ACK/query/freshness tails, decoded and materialized bytes/rows, allocation churn and live peak separately, RSS, whole-job cgroup memory, owned scratch/live storage, cancellation-to-worker-completion and error/refusal counts. For maintenance comparisons record reclaimed-prefix progress and retained-journal byte-time. Do not add independent peak measurements as if simultaneous, and do not multiply isolated gains into a capacity claim.

A kernel's instrumented improvement is insufficient for promotion without the applicable native behavior and resource checks. No result is labeled passed before the actual test ran. Keep failed, interrupted, unavailable and inconclusive outcomes. If the primary candidate does not meet its registered benefit, retain it as research or reject it; an alternative requires a new prospectively registered comparison, not changed budgets on old results.

## Dependency-ordered work packages

All are initially planned. Each implementation mission gets a fully bound manifest from the scaffold before execution. Mutable scopes below are maximum proposed areas, not authorization for every file beneath them; M01 narrows exact inventories.

| ID | Type / class | Depends on | Deliverable and scope | Deciding evidence |
| --- | --- | --- | --- | --- |
| G0 | Release admission gate | Existing first-alpha closeout | Pin actual released artifacts, supported profile and next execution base; preserve current release flow. | Recorded predecessor release identity and independent artifact readback. |
| M01 | `PORT_CONTRACT`, policy | G0 | Scope architecture/depth mapping, mission schema, typed errors, immutable-oracle and gate contracts; separate proposed policy/ADR changes. No runtime code mixed in. | Reviewed protected contracts; adversarial validator fixtures frozen; exact changes and source adaptations accepted. |
| M02 | `PRIMITIVE_OPT`, assurance | M01 | Pure admission/dependency/state validation primitives in existing tooling; no new general agent platform. | Reject missing metadata, renamed-edge violations, fake attributes, cycles, path escape, stale hashes and forged closure controls. |
| M03 | `COMPOSITE_WIRE`, assurance | M02 | Minimal coordinator adapters for attempts, protected verification, leases, receipts, failures and fragments; preserve existing containment. | Tamper/rollback/interruption/three-failure/overlapping-write/forged-receipt controls and cleanup observations. |
| G1 | Baseline gate | M03 | Register workload, cost attribution and exact budgets; measure reference before selecting candidate. | Reproducible baseline identity and measured cost model; no candidate-dependent threshold changes. |
| M04 | `PORT_CONTRACT`, query | G1 | Minimal provided/required interfaces for bounded candidates and delayed materialization; full query/evidence conformance. | Stable tie/snapshot, empty/full output and budget-exhaustion cases; no new public API promise. |
| M05 | `PRIMITIVE_OPT`, query | M04 | Refine or reuse existing safe bounded selector/key path in the narrow admitted scope. | Differential/property invariants, relevant Miri, allocation/instruction budgets, reference retained. |
| M06 | `COMPOSITE_WIRE`, query | M05 | Wire delayed materialization into existing traversal/reader/query paths. | Exact complete page chains, damaged-source/fallback controls, native query improvement and regression checks. |
| M07 | `PORT_CONTRACT`, resource | G1 | Choose lifecycle accounting/admission OR cooperative cancellation; freeze ownership, overflow, progress, errors and limits. | Evidence-supported selection, protected state-machine tests and end-to-end measurement plan. |
| M08 | `PRIMITIVE_OPT`, resource | M07 | Safe checked accounting transitions or cancellation/progress state, not a universal scheduler. | No double release/undercharge/overflow or skipped work; interruption and exhaustion controls. |
| M09 | `COMPOSITE_WIRE`, resource | M08, M06 | Wire actual compute/response ownership and declared adapter boundaries; retain essential progress capacity. | Coupled-load and canceled-client tests, completion-owned release, applicable tail/memory improvements. |
| G2 | Integrated acceptance gate | M06, M09 | Exact candidate packaging, upgrade from released predecessor, required current-profile gates, docs and release readiness. | Candidate-specific receipts, package identity, preserved custody/access, cleanup and explicit remaining limits. |

Independent contract/planning work may proceed when admitted, but shared files serialize. Candidate integration requires revalidation at the actual combined postimage. G2 is a readiness gate; it does not authorize creating a tag.

## Classified recommendation disposition

These references preserve the preceding A-H recommendation taxonomy without turning the entire backlog into release requirements.

| Class | In this milestone | Conditional or deferred |
| --- | --- | --- |
| A. Correctness and release | A1 candidate identity, A2 full-answer equality, A3 actual predecessor upgrade | A4 broader rollback/restore guarantees require separate scope. |
| B. Resources | Choose B1 lifetime accounting or B2 cancellation; B3/B4 only as necessary to implement and validate that choice | Universal allocator/scheduler and whole-service redesign deferred. |
| C. Query | C1 late materialization and existing bounded selection | C2 evidence/catalog reuse is a registered alternative, not an additional mandate; C3 optimizer and C4 rate API deferred. |
| D. Ingestion | Preserve existing delivery; measure ingest as an interference/regression control | D1-D4 commit tuning, fairness, delivery pipelining and ingest-copy redesign deferred unless a real blocker is independently scoped. |
| E. Storage | E3 current scratch/headroom controls are acceptance inputs | E1/E2 new maintenance scheduling and E4 format/compression sweeps deferred. A demonstrated supported-scope defect follows normal blocker policy. |
| F. Operations | F1 minimal bounded cost attribution; existing truthful pressure reporting | Rich F2/F3 interfaces and F4 backup/integrity workflows deferred. |
| G. Assurance | G2 touched lifecycle controls and G3 attributable measurements; G1 narrow extraction only if necessary | G4 automated global evidence-impact engine deferred; use explicit impact records. |
| H. Acceleration | None required | SIMD, Rayon replacement, io_uring/mmap, DataFusion, GPU and distribution remain separately measured research. |

## Definition of done and non-disruption contract

The milestone closes only when its selected missions have externally verified exact postimages; protected contracts/oracles are unchanged except for separately accepted prospective amendments; one query improvement and one selected resource improvement meet their registered rules; the required integrated gates apply to the exact deliverables; the actual predecessor upgrade preserves state; and docs, boundaries, operator limits and verified release-note fragments match implementation.

`Closed` for the planning epic means the engineering objective was accepted, not that a release was automatically published. Final tag/package metadata is `v0.1.0-alpha.2`, Debian `0.1.0~alpha.2`, RPM version `0.1.0` / release `0.alpha.2`, subject to actual package-ordering and upgrade tests. Do not change Cargo package versions merely to mirror release maturity.

No work here retroactively credits a new candidate with old acceptance, changes the existing first-alpha gates, or competes with its admitted heavy trials. A new first-alpha blocker gets its own minimal fix and applicable reruns. A rejected performance idea is not a reason to prolong the current release.

## Implementation limits and primary references

No worker harness, checker extension, optimization, benchmark result or completed gate is supplied by this planning change. All executable manifests remain unadmitted until their actual inputs, commands and budgets are fixed. Current evidence uses the repository's established terminology: implemented, tested, measured and qualified are different claims.

The implementation qualifications above are supported by primary documentation, not by treating the attachment's examples as proof:

- [Rust Reference: inline attributes](https://doc.rust-lang.org/reference/attributes/codegen.html): inlining attributes are hints.
- [Rust Reference: transparent representation](https://doc.rust-lang.org/reference/type-layout.html): representation governs layout/ABI under its stated restrictions.
- [Miri project limitations](https://github.com/rust-lang/miri): checked executions are not a proof of universal soundness.
- [Valgrind Callgrind manual](https://valgrind.org/docs/manual/cl-manual.html): instruction attribution and optional cache/branch simulation.
- [Cargo metadata](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html): package IDs, dependency kinds and resolved graph are distinct data.

Results and research interpretations belong in the existing research wiki. Registered contracts, protocols and machine-consumed inputs remain in the repository. The owner-supplied blueprint's unsupported quantitative research assertions are intentionally not republished as project findings.
