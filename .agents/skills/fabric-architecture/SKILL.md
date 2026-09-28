---
name: fabric-architecture
description: Maintain Fabric O11y's architecture documentation from repository evidence when component boundaries, behavior, contracts, diagrams, or architectural decisions change, or when a documentation audit is requested.
---

# Maintain the architecture model

Read [AGENTS.md](../../../AGENTS.md), [CURRENT.md](../../../docs/CURRENT.md), the [product contract](../../../docs/PRODUCT-CONTRACT.md), and the relevant view from the [documentation landing page](../../../docs/README.md), respecting its source-of-truth order. Then inspect the source, build metadata, and existing evidence for the affected boundary.

Distinguish implemented behavior, an accepted decision, a future proposal, and an unverified assumption. The [blueprint](../../../docs/architecture.md) is a design agenda, not evidence that a mechanism exists. Record a discrepancy before deciding whether code or documentation needs correction; a documentation-only request does not authorize implementing the blueprint.

Update only the pages, concepts, glossary entries, and diagrams affected by the request. Keep the canonical `.mmd` and any marked Markdown copy identical. Use ordinary relative links. Create an ADR only for a material decision; state when its original rationale or comparative evidence is unknown. Update the [verification matrix](../../../docs/formal/verification-matrix.md) when a claim or its checks change, and `CURRENT.md` as a concise description of present state.

Run `cargo xtask check-layers` when crate boundaries change and `bun tools/docs/check.mjs` from the repository root. When changing validation itself, run `bun tools/docs/check.test.mjs`; for hooks, run `python3 -B tools/docs/test_hooks.py`. Inspect actual exit codes and repair relevant failures. The checker establishes links and syntax, not semantic agreement with Rust.

Finish with the affected boundary, supporting source paths, checks actually run, and any unresolved divergence. Follow the [contributor guide](../../../docs/CONTRIBUTING.md) for tooling and hook activation.
