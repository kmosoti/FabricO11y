# ADR-0007: Preserve multiple time axes and explicit relations

Status: Accepted

## Context

Distributed producers have skewed clocks, late delivery, local sequences, and causal information
that cannot be represented honestly by one timestamp.

## Decision

Preserve source observation time, collector observation time, durable admission time,
producer-local sequence, named logical time, and parent/link/correlation relations as distinct
fields. Compare records only when an explicit strong signal applies; otherwise retain concurrent
or unknown ordering.

## Consequences

Replay can use stable causal and producer-local evidence without rewriting source time. Callers
must handle partial-order outcomes instead of relying on a convenient timestamp sort.

## Rejected alternatives

- Sort all records by wall-clock time: rejected because clock skew fabricates order.
- Collapse every relation into a trace parent: rejected because correlation and dependency do not
  establish parentage.

## Supersession

Supersedes no prior decision.
