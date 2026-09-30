# AGENTS.md

Operational contract for coding agents in FabricO11y. Humans follow the same rules through the [contributor guide](docs/CONTRIBUTING.md). The full [documentation policy](docs/documentation-policy.md) applies to every change that touches architecture or documentation.

## Starting point

Before a substantial change, read:

1. [docs/CURRENT.md](docs/CURRENT.md): present state, outstanding work, risks.
2. [docs/PRODUCT-CONTRACT.md](docs/PRODUCT-CONTRACT.md): what is promised.
3. The relevant [architecture view](docs/architecture/README.md) and its diagrams.
4. The relevant [ADRs](docs/decisions/README.md).
5. The claim's row in the [verification matrix](docs/formal/verification-matrix.md).
6. The affected source and its tests.

Source-of-truth order: product contract, accepted ADRs, architecture views, registered protocols and results, current state, then research ([hierarchy](docs/README.md#source-of-truth)). The [blueprint](docs/architecture.md) is a proposal, not behavior. Code is evidence of what exists; reconcile a disagreement explicitly instead of silently editing either side.

## Architecture rule

Domain semantics point inward; effects point outward ([ADR-0015](docs/decisions/ADR-0015-adopt-a-hexagonal-architecture.md)). Layers: core, ports, app, adapter support, adapters, composition roots. [docs/architecture/layers.json](docs/architecture/layers.json) assigns every crate; `cargo xtask check-layers` rejects any forbidden edge, including renamed, optional, target-specific, build, development and transitive ones. A new crate needs a layer; an exception needs a written reason. Do not add generic repository traits, service locators, plugin systems, event buses or dependency-injection frameworks; add a port only where an effect crosses a meaningful boundary.

## Core purity rule

`fabric-core` decides what a transition means from explicit inputs only, and returns effects as data ([ADR-0016](docs/decisions/ADR-0016-keep-a-pure-semantic-core.md)). It is `no_std`, forbids unsafe code, has no dependencies, and must not read the clock, environment, filesystem, network, process state, randomness or global mutable state. `cargo xtask check-core-purity` enforces the dependency allowlist and structural guards. A new core dependency is an explicit scope item and a policy change, never incidental. Purity does not decide ownership: a pure codec may still belong to adapter support.

## Generator-verifier rule

An agent proposes; the repository verifies ([ADR-0018](docs/decisions/ADR-0018-accept-work-on-executable-evidence.md)). Your own prose is not evidence. Tests you generate alongside an implementation are not independent oracles. The Python delivery, query and rate oracles stay independent of the Rust code. No language-model review or approval is a completion gate.

## Specification protection

A patch that changes the product contract, an oracle, a negative control's expected outcome, a formal invariant, a registered protocol or verification policy (including `xtask/checks.json`, `xtask/mutants.json`, the layer and purity policies) is a trust-boundary change. Put it in its own commit with its reason; never mix it into an implementation change, and never weaken a check to make an implementation pass.

## Evidence rule

Never write "passed", "proved" or "qualified" unless the corresponding check ran and supports that exact claim. Report the real command and exit status. Report "not run", "interrupted", "failed", "inconclusive" and "environment unavailable" as such. A historical result belongs to the revision it ran on. Run the fast checks with `cargo xtask checks --profile fast`; it writes receipts under `target/verification/receipts`.

## Counterexample rule

Keep meaningful failures. Minimize a defect found by an oracle, mutant, property test, fuzzer, model checker, fault run or review into a deterministic regression fixture recording its origin, seed or trace, the contract it violated and the fix. When introducing a checker, inject a representative defect it must reject.

## Branch rule

Work on `milestone/<capability>` branches ([roadmap](docs/ROADMAP.md)); never name branches or documents after pull-request numbers, model names, sessions or release maturity. Release maturity lives only in tags and package metadata ([ADR-0019](docs/decisions/ADR-0019-keep-release-maturity-in-tags.md)). Use plain Git; preserve existing work; never rewrite published history or force-push.

## Scope rule

These need explicit task scope: protocol or wire-format changes, durability or sync-order changes, verifier or oracle changes, new core dependencies, destructive fault runs, privileged installation, qualification runs, releases and tags. A stopped or interrupted run stays so until a new run is recorded. Do not advance into a new milestone because the blueprint mentions it.

## Working habits

- Keep changes small and explainable; explain the Rust idea, the contract and the trade-off before introducing a mechanism, and keep the [learning path](docs/LEARNING_PATH.md) in step.
- Update code, tests, diagrams, architecture pages, glossary, ADRs and `CURRENT.md` in the same logical change.
- State assumptions for correctness arguments; register workloads and metrics before performance comparisons.
- Use the cheapest capable agent for separable work; avoid fast or priority modes unless asked. Do not ask agents for private chain-of-thought; ask for tool and activity evidence.

## Commands

- Run the demonstration: `cargo run --offline`.
- Fast checks with receipts: `cargo xtask checks --profile fast`.
- Individual gates: `cargo xtask check-layers`, `cargo xtask check-core-purity`, `cargo xtask mutants`.
- Verification tools ([contributor guide](docs/CONTRIBUTING.md#verification-tools)): `cargo test -p fabric-properties`, `bash formal/kani/check.sh`, `bash fuzz/smoke.sh 60`, `cargo test -p fabric-sim`, `cargo deny --locked check`; the extended profile runs them all: `cargo xtask checks --profile extended`.
- Documentation tooling (once): `bun install --cwd tools/docs --frozen-lockfile`; then `bun tools/docs/check.mjs`, `bun tools/docs/check.test.mjs`, `python3 -B tools/docs/test_hooks.py`.

Reusable workflows live in `.agents/skills`: [fabric-architecture](.agents/skills/fabric-architecture/SKILL.md), [fabric-lesson](.agents/skills/fabric-lesson/SKILL.md), [fabric-experiment](.agents/skills/fabric-experiment/SKILL.md). Hooks load context and check documentation; they do not decide whether prose is true.
