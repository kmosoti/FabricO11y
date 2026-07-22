# ADR-0002: Separate evidence envelopes

Status: Accepted

## Context

Observations, corrections, completeness claims, and derived answers have different invariants and
revision behavior. A single permissive event shape would allow consumers to blend those meanings.

## Decision

Use four versioned, closed envelopes: observation, correction, frontier, and answer. Each has a
stable `fabric.<kind>/v1` identifier and normative Draft 2020-12 schema. Corrections are append-only
records, and answers must carry their support and limits.

## Consequences

Bindings and storage must retain envelope identity. Contract changes need compatibility analysis
and fixtures. Conversion code is more explicit, while invalid mixed states fail earlier.

## Rejected alternatives

- One generic event envelope with optional fields: rejected because incompatible states become
  representable.
- Treating corrections as in-place updates: rejected because it destroys replayable history.

## Supersession

Supersedes no prior decision. A later accepted ADR may supersede this record only with an explicit
envelope migration.
