---
name: architecture-review
description: Review consequential FabricO11y architecture, algorithm, storage, query-engine or dependency choices against feasible alternatives and revision-bound evidence. Use for a requested review or an unresolved material design decision, not routine fixes or already-decided local edits.
---

# Review a consequential design choice

Determine whether a design earns its complexity and which evidence could change
the decision. Respect the request's scope: a review does not authorize an
implementation, dependency adoption, contract change or experiment execution.
Existing authorization still applies; do not create another approval gate.

## Establish the decision and baseline

Read [AGENTS.md](../../../AGENTS.md) and its required starting context for the
affected boundary. State the decision, acceptance criteria, invariant contracts,
representative workload and allowed changes. The product contract and accepted
ADRs define normative obligations in the repository's source-of-truth order;
source and executable observations establish what exists. Record a disagreement
explicitly rather than silently treating either code or a historical summary as
permission to revise a promise.

Record the full baseline Git SHA and dirty state, including relevant staged,
unstaged and untracked work. Identify any relevant diff or source snapshot and
the exact artifacts used by the review: paths plus content hashes for retained
inputs, protocols, datasets, receipts and candidate binaries/packages where
applicable. Tie a historical result to its own revision and artifact identities;
an unavailable artifact remains unavailable, even if its summary is readable.
Search relevant symbols and paths before opening large files. Inspect the
smallest source/test/evidence slice that resolves a consequential uncertainty.

## Compare practical alternatives

Compare the current design with the strongest feasible alternatives. Consider a
simpler mechanism and an established implementation when they bear on the
decision; do not manufacture a fixed number of candidates. A named engine or
library is a comparison candidate, not permission to adopt an excluded dependency.
Identify contract conflicts before ranking candidates.

Compare semantic coverage, correctness and failure behavior, complete-operation
resource costs, operational ownership, maintenance/dependency cost, migration
and reversibility. For algorithms, examine complexity and data distribution;
for engines, include decoding, admission, intermediate state, sorting and
response construction where relevant. Distinguish raw custody from query
projections, scan completeness from collection coverage, and returned-row limits
from bounds on total work. Optimize only within equivalent required semantics.

Separate source facts, deductions, estimates and measured outcomes. Check
version-sensitive external claims against primary upstream sources. Vendor
benchmarks and invented scores cannot establish a matched-workload ranking.
State each consequential untested premise and what observation would falsify it.

## Resolve only the uncertainty that matters

Use `multidimensional-reasoning` for genuinely ambiguous designs with interacting
constraints or materially different representations; do not duplicate its
exploration workflow here. Route a needed registered measurement or formal
investigation through [fabric-experiment](../fabric-experiment/SKILL.md), including
competent, semantically matched baselines and a decision rule fixed before runs.
Keep planning separate from executed evidence. All execution remains subject to
AGENTS.md containment, storage, cleanup and scope rules.

Report performance only for the measured workload, configuration, environment
and finite bounds. State a security claim's threat boundary, assumptions and
checks actually exercised. Neither a benchmark nor passing security tests imply
universal performance superiority, arbitrary-input resource bounds or absence
of vulnerabilities. A model result applies only to its stated model and bounds.

Lead with the recommended action, then the decisive evidence and trade-offs.
Say when alternatives remain unresolved; propose the smallest discriminating
observation or experiment only if needed. Include actual commands/outcomes,
artifact references and material limits, or label verification not run. Stop
optional exploration when it no longer affects the decision while continuing
any outstanding authorized queue. Route accepted architecture/documentation
changes through [fabric-architecture](../fabric-architecture/SKILL.md); keep
research/results in the home required by the documentation policy. A model
review is advice, never an executable acceptance gate.
