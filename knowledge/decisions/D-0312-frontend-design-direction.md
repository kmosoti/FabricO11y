# D-0312 · Workflow-first frontend design direction

Date 2026-11-27. Status proposed. This record captures an initial design
hypothesis for issue [#312](https://github.com/kmosoti/FabricO11y/issues/312).
Human review and browser qualification remain open. The stable interaction
contract is in [docs/design.md](../../docs/design.md).

## Evidence basis

Issue #312 asks for one subject-specific visual language and three distinct
compositions evaluated with a shared synthetic fixture and task script. The
repository describes optional frontends behind authorized read contracts in
[docs/architecture.md](../../docs/architecture.md#optional-frontend-boundary)
and [D-0192](D-0192-optional-frontend.md). The backend and CLI stay headless by
default. A complete frontend design system remains to be defined.

Current contracts supply important meaning to preserve. `fabric-evidence`
snapshots retain known and unknown case outcomes and compare compatible rosters,
units, aggregations and references. They report numeric differences without a
claim of significance or causality. The `fabric-colloquium` projection holds
bounded attribution claims. Actor, principal and team IDs denote attribution,
while adapters establish authenticated identity. Custody, projection visibility
and retention availability remain separate states.

## Shared fixture and tasks

Use one synthetic fixture in all three alternatives. It contains a searchable
run, a parent and child revision, two comparable snapshots with declared scope,
known, unknown and partial cases, evidence references with source, digest,
readback and availability fields, plus a clearly labeled local prototype field.
Prototype state and evidence state remain separate in every view.

Use the same task sequence on wide desktop and narrow mobile storyboards. Find a
named run or revision. Inspect its source, time and result. Follow its parent or
lineage. Open an evidence reference and report what its state establishes.
Compare the two named revisions while retaining unknowns. Return to the original
search and selection with context preserved. Storyboard frames are design
targets, with browser viewport testing still pending.

| Composition | Wide desktop | Narrow mobile | Task fit and tradeoff |
| --- | --- | --- | --- |
| Split evidence desk | Results, evidence and lineage occupy adjacent regions. | One region fills the view while tabs or drill-down preserve the path. | Evidence sits near results. Narrow navigation increases context changes and can weaken chronology. |
| Chronological investigation ledger | An ordered investigation stays central with a persistent record inspector. | The ordered thread fills the view, and inspection uses a focused view with a return path. | Sequence and lineage support the full find, inspect, follow, compare and return task. Pairwise comparison needs a separate mode, and narrow inspection leaves the thread temporarily. |
| Paired comparison workbench | Two named inputs and their evidence align side by side. | Matched sections stack in a named pair. | Scope and differences stay together for pairwise review. Search and lineage browsing become secondary, and unequal inputs can appear equivalent without clear scope. |

## Provisional direction

Carry the chronological investigation ledger forward as the leading hypothesis,
with a paired comparison mode. Its rationale is continuity across the same
investigation tasks and alignment with the product's time and lineage semantics.
This rationale predicts task fit without rendered or measured comparison
results. Retain the evidence desk and comparison workbench as live alternatives.

Reconsider the ledger if a later matched evaluation shows loss of source scope,
evidence state or return context across desktop and narrow layouts. Human design
review, rendered comparison, token definitions, component states, browser and
resource qualification, and automated plus manual WCAG 2.2 AA review remain
open. Palette, font, token values, screenshot results, task success rates and
accessibility conformance remain unestablished.
