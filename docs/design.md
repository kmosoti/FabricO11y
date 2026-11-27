# Optional frontend interaction contract

The backend stays headless by default, and the `fabric` CLI remains a complete
operating path. An optional frontend presents authorized query results through
the shared backend contracts. Authorization, custody and projection status
remain properties of their existing owners.

## Investigation flow

The interface supports a continuous investigation. Operators move from search
or command entry through chronological results, record inspection, lineage,
evidence and explicit comparison. A return action restores the prior query,
filters, selection and reading position.

On a wide display, keep the chronology as the main reading surface and place a
stable inspector beside the selected record. Show parent and child relations as
a path attached to that record. Keep comparison as an explicit mode with two
named inputs and visible scope.

On a narrow display, keep the same information order. Show the chronology at
full width, open a focused record view, and provide a clear route back to the
same result context. Preserve comparison scope and the selected inputs across
reflow.

## Evidence and state

Keep local prototype values separate from evidence references. Label local
prototype fields with their `prototype` source. Show evidence references with
their source identity, digest and validation boundary when those values are
available. Custody, projection visibility, retention availability, snapshot
outcome and prototype state answer different questions. A receipt can be
committed while projection remains pending, and retention can later remove a
queryable row. Actor, principal and team identifiers from `fabric-colloquium`
remain attribution claims. The backend establishes identity through its
authentication boundary.

Use the following words consistently:

- **Pending** — Work or review still awaits a final result.
- **Unknown** — Available information cannot establish a value.
- **Missing** — An expected item or reference is absent within its declared scope.
- **Partial** — Some of the declared scope is represented and the remainder is unresolved.
- **Verified** — A named check passed for the identified item and scope.
- **Conflict** — Declared sources disagree or a stated constraint fails.

Pair each state with a readable label and a structural marker. Color may reinforce
meaning, while text and structure carry the state on their own. Name the check
behind `Verified` so readers know the boundary it covers. Preserve unknown and
missing outcomes through summaries and comparisons.

`fabric-evidence` snapshot comparisons require a matching roster, unit,
aggregation and reference set. They preserve unknown outcomes and report
numeric differences without establishing significance or causality. Keep raw
observations, sketch summaries and analyst claims visually and semantically
distinct.

## Responsive and accessible behavior

Use semantic design roles and component states to organize future visual
tokens. Keep content reflow, keyboard operation, visible focus, reduced motion,
high contrast and non-color state cues in the component contract. Long IDs,
prose and numerical evidence need readable wrapping and explicit density
controls. Native browser layout remains responsible for responsive flow.

This contract leaves palette, typography, token values, renderer selection and
WCAG conformance open for human review and rendered qualification. The optional
frontend boundary and its dependency requirements are described in the
[architecture](architecture.md#optional-frontend-boundary).
