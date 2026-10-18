---
name: architecture-review
description: Review consequential FabricO11y architecture, algorithm, storage, query-engine or dependency choices against feasible alternatives and revision-bound evidence. Use for a requested review or an unresolved material design decision, not routine fixes or already-decided local edits.
---

# Review a consequential design choice

Decide whether a design earns its complexity and what could change the recommendation. A review does not authorize implementation, dependency adoption, contract changes or experiment execution. Existing authorization persists; add no approval gate.

## Establish the decision and baseline

Read [AGENTS.md](../../../AGENTS.md) and its required context. State the decision, acceptance criteria and invariants. The product contract and accepted ADRs define obligations; source and executable observations show current behavior. Record disagreements explicitly.

Capture the full Git revision and dirty state, including staged, unstaged and untracked changes. Pin relevant source snapshots and retained artifacts by path and content hash. Tie historical results to their own revision and artifact identities; unavailable artifacts remain unavailable. Search relevant symbols first and inspect the smallest evidence slice that resolves the decision.

## Compare feasible choices

Compare the design with the strongest practical alternatives, including simpler or established approaches when relevant. A named library is a comparison candidate, not authorization to adopt it. Check contract conflicts, then compare semantic coverage, correctness, failure behavior, complete-operation resource use, maintenance cost, dependencies, migration and reversibility. Match semantics and resource conditions before comparing performance. Keep raw custody distinct from query projections, storage completeness from collection coverage, and returned-row limits from bounds on total work.

Separate observed facts, documented behavior, deductions, estimates and measurements. Check changing external claims against primary documentation. Name consequential assumptions and uncertainty, plus the observation that would falsify the leading explanation or recommendation.

## Resolve and report

Use multidimensional-reasoning for ambiguous designs with interacting constraints. Route registered measurements or formal investigations through [fabric-experiment](../fabric-experiment/SKILL.md); distinguish plans from executed evidence. Route architecture or documentation changes through [fabric-architecture](../fabric-architecture/SKILL.md). Follow AGENTS.md containment, storage, cleanup and scope rules for authorized execution.

Limit performance conclusions to the measured workload, configuration, environment and bounds. State security threat boundaries, assumptions and checks exercised. Finite results do not establish universal speed, arbitrary-input resource bounds or absence of vulnerabilities. A model conclusion applies only within its stated model and bounds.

Lead with the recommendation and decisive evidence. Include commands and outcomes, artifact references and material limits, or say verification was not run. Identify unresolved alternatives and the smallest useful next observation. A model review is advice, never an executable acceptance gate.
