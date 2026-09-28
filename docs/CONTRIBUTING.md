# Contributing

Start with [CURRENT.md](CURRENT.md), the [product contract](PRODUCT-CONTRACT.md), the affected [architecture view](architecture/README.md) and the source. [AGENTS.md](../AGENTS.md) holds the same rules for coding agents; the [documentation policy](documentation-policy.md) applies to everyone.

## Architecture

Code lives in layers: `fabric-core` (pure `no_std` decisions), `fabric-ports` (effect contracts), `fabric-app` (use cases), `fabric-frame` (adapter support), and the composition roots `fabric-server` and the root package ([system view](architecture/system.md)). `cargo xtask check-layers` and `cargo xtask check-core-purity` enforce the dependency rule and the core's purity ([ADR-0015](decisions/ADR-0015-adopt-a-hexagonal-architecture.md), [ADR-0016](decisions/ADR-0016-keep-a-pure-semantic-core.md)). Wire and persisted bytes never change with a type move.

## Milestones, branches and releases

Work belongs to a milestone and its `milestone/<capability>` branch ([roadmap](ROADMAP.md)). Release maturity is only a tag such as `v0.1.0-alpha.1` on a revision whose required gates have recorded evidence ([ADR-0019](decisions/ADR-0019-keep-release-maturity-in-tags.md)). Never rewrite published history.

## Evidence and verification

Acceptance rests on executable evidence, not on anyone's approval ([ADR-0018](decisions/ADR-0018-accept-work-on-executable-evidence.md)); the [verification strategy](formal/verification-strategy.md) says which technique answers which question.

- The Python delivery, query and rate oracles are independent of the Rust code and stay that way. A test written with an implementation is not an independent oracle.
- Changing the product contract, an oracle, a negative-control expectation, a formal invariant, a registered protocol or verification policy is a trust-boundary change: its own commit, with its reason.
- A checker ships with a defect it must reject. Semantic mutants in [xtask/mutants.json](../xtask/mutants.json) run with `cargo xtask mutants`; no unexplained surviving mutant may threaten a claimed contract.
- A defect found by any means becomes a minimized, deterministic regression fixture.
- Say "passed" only for a check that ran; report not run, interrupted, failed, inconclusive and environment unavailable plainly.

## Checks

`cargo xtask checks --profile fast` runs every required fast check listed in the [registry](../xtask/checks.json) and writes receipts under `target/verification/receipts`; a missing tool yields INCOMPLETE (exit 3), never a pass. The `extended` profile adds the semantic mutants, delivery fault runs and TLA+ models. Registered qualification protocols ([qualification](QUALIFICATION.md)) never run in CI; they need explicit scope. [Rust CI](../.github/workflows/rust.yml) and [documentation CI](../.github/workflows/docs.yml) run the fast set.

Documentation tooling needs Bun 1.4.0 and Python 3: `bun install --cwd tools/docs --frozen-lockfile`, then `bun tools/docs/check.mjs`. The [checker](../tools/docs/check.mjs) verifies relative links and heading fragments, Mermaid syntax, marked diagram copies, JSON syntax and duplicate ADR numbers; it performs no network access or writes and does not decide whether prose is true. Canonical diagrams live in `docs/diagrams/*.mmd`; a page may repeat one after a `<!-- diagram: relative/path.mmd -->` comment, and the checker requires the copy to match. When changing the checker or hooks, also run `bun tools/docs/check.test.mjs` and `python3 -B tools/docs/test_hooks.py`.

## Skills and hooks

Repository skills in `.agents/skills` are ordinary Markdown: [fabric-architecture](../.agents/skills/fabric-architecture/SKILL.md), [fabric-lesson](../.agents/skills/fabric-lesson/SKILL.md) and [fabric-experiment](../.agents/skills/fabric-experiment/SKILL.md). [.codex/hooks.json](../.codex/hooks.json) runs [read-only handlers](../.codex/hooks/run.py): `SessionStart` supplies a bounded copy of `CURRENT.md`; `Stop` runs the documentation checker and reports missing tools as skipped, never as success. Hooks need runtime trust in Codex (`/hooks`); without it, run the commands by hand. The same configuration holds silent [agent-telemetry](../tools/telemetry/README.md) observers that write under ignored `.local/`; validate them with `python3 -B tools/telemetry/test_contract.py` and `python3 -B tools/telemetry/test_cli.py`.

## Editor tools

[.vscode/extensions.json](../.vscode/extensions.json) recommends [Foam](https://marketplace.visualstudio.com/items?itemName=foam.foam-vscode), [Mermaid Preview](https://marketplace.visualstudio.com/items?itemName=vstirbu.vscode-mermaid-preview), [Mermaid Visual Diff](https://marketplace.visualstudio.com/items?itemName=orhymed.mermaid-visual-diff) and [CodeTour](https://marketplace.visualstudio.com/items?itemName=vsls-contrib.codetour). None is required; all navigation uses ordinary Markdown links.
