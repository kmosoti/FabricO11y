# Evidence Semantics Standard

Status: Normative for `fabric.*/v1` envelopes.

## Envelope identities

The stable envelope identifiers are `fabric.observation/v1`, `fabric.correction/v1`,
`fabric.frontier/v1`, and `fabric.answer/v1`. The corresponding files in `schemas/` are the
machine-readable source of truth. Storage and transport representations must preserve these
envelopes rather than substitute their own meaning.

## Observations and epistemic class

An observation is evidence that a producer emitted or a collector observed a record. Admission is
not endorsement of the payload's interpretation. Every observation declares exactly one
`epistemic_class` and a matching, closed `interpretation` object:

- `observed_fact` records a concrete emission, reading, or action report;
- `derived_conclusion` names its derivation and input records;
- `correlation` associates at least two records without implying causation;
- `assumption` declares an unestablished premise in plain language.

The class and interpretation kind must match. Consumers must reject an omitted, mixed, or unknown
class and must not infer class from payload fields.

## Time and ordering

The axes below are independent signals:

- `observed_at` is source-clock time and may be null;
- `observed_by_at` is collector/runtime receipt time;
- `recorded_at` is durable FabricO11y admission time;
- `producer_sequence` is a producer-and-stream-local position;
- `logical_time` is a named logical clock position.

Parent, link, and correlation relations are separate arrays. Parentage is an explicit causal or
structural claim; a link is a non-parent dependency association; correlation carries no causal
claim. Comparability follows explicit relations, a shared logical clock, or a shared producer
stream sequence. Wall-clock timestamps alone never manufacture a total order.

## Corrections

Corrections are append-only evidence. They never mutate or erase their targets:

- retraction removes a target from active support;
- replacement supersedes targets with identified replacement observations;
- qualification retains a target while recording a narrower interpretation;
- deduplication names one canonical record and marks its other targets as duplicate support.

Every correction target must resolve during replay; every `replacement_ids` reference on a
replacement or deduplication correction must resolve as well. A reference is forward when the
correction is `Before` the referenced observation under the partial order. Forward and missing
references are deterministic semantic errors, not implicit placeholders. `Concurrent` and
`Unknown` relations are not forward,
and replay iteration or `recorded_at` does not override a stronger relation.

When several correction kinds target the same observation, the materialized disposition uses the
stable precedence `retracted > replaced > duplicate > active`. Qualifications accumulate
independently and all corrections remain in history. Distinct replacement sets or distinct
deduplication canonicals are retained as explicit semantic conflicts; deterministic replay does not
silently choose one claim as true.

## Frontiers and answers

A frontier is a producer-and-stream completeness claim as of a durable cutoff. It keeps known
sequence gaps, retention start, sampling state, and producer state explicit. It does not assert
that another producer is complete. Frontiers carry classification because they are admitted
metadata and must cross the same pre-persistence policy boundary as other evidence.

An answer includes its result and the evidence limits needed to revise it: provenance, source
cutoff, frontier references, missing sources, coverage, assumptions, conflicts, and derivation.
Empty arrays mean "evaluated and none declared"; a missing required field is invalid.

## Versioning and compatibility

Envelope versions are part of `api_version` and the schema `$id`. Additive changes are compatible
only when existing valid v1 instances remain valid and existing consumers may safely ignore the
new optional field. Removing a field, changing meaning, narrowing an accepted value, adding a
required field, or changing correction/order semantics is breaking and requires `/v2` plus a
compatibility note and new immutable fixtures.

Golden fixtures already referenced by a compatibility result are not overwritten. A correction
adds a new fixture version. Schema updates require positive and intended-negative fixtures and the
checked-in validation gate.
