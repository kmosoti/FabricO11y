# Console art direction

Status: working design direction, not a usability or accessibility conformance
claim. `fabric-ui` contains bounded display models and a compiled demonstration;
task evaluation and authenticated product integration remain separate gates.

## Evidence and judgment

- **Observed contract:** operators inspect telemetry, narrow by time/source,
  explore logs/metrics/spans, and understand gaps, freshness, completeness and
  recovery ([milestone](../milestones/operator-console.md)).
- **Observed limits:** tail ≤4,096 rows/1 MiB; charts ≤65,536 inputs/1,024 buckets.
  Reduction retains extrema/gaps, not a complete dataset or query aggregate
  ([model](console-algorithms.md)).
- **Research guidance:** follow the domain task/data through abstraction,
  encoding and implementation; evaluate at the level of the claim
  ([Munzner, 2009](https://www.cs.ubc.ca/labs/imager/tr/2009/NestedModel/)).
  Visual encoding affects interpretation and deserves task checks
  ([Heer and Bostock, CHI 2010](https://homes.cs.washington.edu/~jheer/files/2010-MTurk-CHI.pdf)).
- **Standards guidance:** target WCAG 2.2 AA and manually check context. Charts
  need meaningful text equivalents and data access ([WCAG 2.2](https://www.w3.org/TR/WCAG22/),
  [USWDS visualization](https://designsystem.digital.gov/components/data-visualizations/));
  tables need captions, consistent units, narrow-screen and keyboard support
  ([USWDS table](https://designsystem.digital.gov/components/table/)).
- **Design judgment:** a quiet, instrument-like interface suits comparison,
  evidence inspection and uncertainty triage. Evaluate this choice; it is not a
  universal scientific claim about visual style.

## Direction

The main path is **observe ingestion → narrow time/source → inspect log, metric
or trace evidence → triage gaps → supported recovery action**. Each screen should
answer “what am I seeing, for which source/interval, how complete is it, and
what can I safely do next?” Keep source/time visible through detail.

Use the requested cyan identity as a restrained accent on white/light and deep
navy/dark surfaces. Establish semantic tokens rather than styling each widget
independently:

| Token role | Use |
| --- | --- |
| `canvas`, `surface`, `surface-raised` | Page, work area and occasional grouped region; most content stays on the canvas. |
| `text-primary`, `text-secondary`, `text-muted` | Body, supporting labels and low-priority metadata; all readable at normal zoom. |
| `accent`, `accent-strong` | Selected navigation, links and primary actions; cyan is not the only signal for state. |
| `border`, `focus-ring` | Quiet separators and a distinct, visible keyboard focus indicator. |
| `healthy`, `stale`, `incomplete`, `error` | Operational states paired with text and/or a shape/icon, never color alone. |
| `series-*` | A small, stable set of chart series colors, supplemented by labels or line/marker styles. |

Check actual values for text contrast (4.5:1 ordinary, 3:1 large text and
meaningful graphical objects where WCAG applies), focus visibility, both themes
and common color-vision differences. Avoid failing cyan body text. Glow, gradient
and shadow must not carry primary hierarchy; reserve elevation for overlays.

Use system sans-serif for interface text and tabular/monospaced numerals for
timestamps, IDs and numeric values. Keep few explicit type roles, sentence-case
labels, right-aligned numeric columns, stable units and exact values available
when compact displays are rounded.

Use a 4 px spacing base (4/8/12/16/24/32), thin borders and whitespace. Prefer a
dominant work region and a few context groups over repeated equal-weight cards.
Keep density sufficient for comparison without making every value a headline.
Use the specified desktop sidebar (Home, Explore, Traces, Pipeline, Settings)
and mobile bottom navigation; keep Settings reachable. Wrap/stack filters and
details on narrow screens instead of shrinking data labels.

## Evidence presentation rules

- Give charts title, units, source/series, time range, readable scale/ticks and
  text summary. Provide accessible values too; a table alone does not express
  interpretation. Keep empty/loading/error states nearby.
- Keep scale policy explicit. Start rates/counts at zero unless a marked baseline
  is necessary; never silently change comparable scales. Preserve integer
  timestamps/IDs without lossy JavaScript-number conversion.
- A bucket may show observed min/max. Do not connect buckets, interpolate gaps,
  imply samples in an empty bucket, or call the envelope an average/complete
  history. Label the reduction, selected range and source.
- Separate **no matching records**, **no samples**, **explicit gap**,
  **partial answer**, **stale observation** and **query failure**. Do not collapse
  these into one empty state or generic green/red badge.
- Name producer, unit, sampling window, reset and unavailable/stale behavior.
  Keep accepted batches, committed records, projected rows, bytes, backlog and
  acknowledged custody distinct. Render only API-supported metrics/actions.
- Use semantic tables with caption/source/update context, headers, stable units
  and mobile stack/keyboard-scroll behavior. Do not compress timestamps, source
  identity or evidence status into unreadable abbreviations.
- Make actions say what they change and show the resulting state. Disabled,
  unavailable and not-yet-implemented controls must not look actionable. Confirm
  destructive recovery actions with their actual scope and consequence.
- Support keyboard, visible focus, logical headings, reduced motion, zoom/reflow
  and WCAG 2.2 touch targets. Announce async status without repeated interruption.

## Art direction flow and acceptance

1. **Task frame:** state operator goal, source, interval, signal, uncertainty and
   action; check the product contract before drawing.
2. **Evidence map:** list fields/actions in the query/control contract; mark gaps,
   partial/stale values and unsupported operations.
3. **Hierarchy:** sketch navigation, query context, evidence, completeness and
   next action at desktop/mobile widths; group common triage comparisons.
4. **Encoding:** choose table/chart/detail; specify units, scale, labels,
   color/shape semantics and nonvisual equivalent before CSS.
5. **Tokens:** apply shared type, spacing, color, border and control states;
   inspect both themes and real content lengths.
6. **Walkthrough:** use deterministic normal, spike, empty, gap, stale, partial,
   long-source and failure/recovery fixtures; check correct interpretation.
7. **Finite task evaluation:** prerecord task, operator profile, fixture, viewport,
   measures and decision rule before comparing variants. Measure success, wrong
   interpretations/actions, time and accessibility barriers, never a beauty score.

Completion: operator identifies source/interval, distinguishes missing/empty and
stale/current, finds evidence, understands units/scale, accesses it without color
or sight, completes the narrow-width keyboard task and sees only supported actions.
Record failures with their fixture.

## Measurement model

For a fixed investigation, model completion time as
`T = T_navigation + T_query_wait + T_interpretation + T_action`.
This is a measurement decomposition, not a fitted prediction. Faster chart
preparation reduces only part of this total; it does not establish faster or more
accurate investigations. A visually attractive view can still increase the
interpretation term or the probability of a wrong conclusion.

Compare designs subject to evidence fidelity, authorization, resource bounds and
accessibility requirements. Then measure task success, wrong interpretations,
wrong actions, median/tail completion time and assistance required. Keep these
dimensions separate instead of inventing a weighted beauty/performance score.
For example, the gap task asks whether an interval contains zero measurements
or unavailable evidence; the correct fixture answer exists independently of
the screen being evaluated.

Before a human comparison, register participants' operator experience, task and
fixture difficulty, viewport, browser, order and stopping rule. Counterbalance
variant order and use equivalent fixtures to reduce learning effects. Report
failures and uncertainty with timings; a small formative study finds problems
but does not estimate a population-wide speedup. Automated fixture checks verify
values and interactions, not human comprehension or screen-reader usability.

## Generic-dashboard failure checks

“AI tells” here means familiar generic dashboard shortcuts, not authorship
detection or a scientific style test. Flag invented metrics, decorative/random
sparklines, empty controls, emoji icons, illegible cyan labels or glowing cards
when they obscure evidence, misstate capability, harm access or hinder comparison.
Name the task and verify with completion checks; style preference alone is not
proof of usability failure.
