# Pure alert lifecycle

A dependency-free Sans-I/O leaf keeps evaluation out of the collector dependency
closure. A rule binds one normalized integer profile, source, instance and
revision. Inclusive thresholds use strict hysteresis ordering; evaluation time
and source frontier are caller supplied. Covered intervals establish continuity
only where they overlap the preceding covered evaluation. Holds start at the
first evaluated point, without retroactive credit.

Missing, stale, partial, unavailable, error or denied input breaks a hold and
cannot clear a firing instance. A later missing evaluation may retain the source
frontier; covered values must advance it. Exact duplicates return the prior
decision and conflicting or older inputs fail before mutation. The latest typed
input retains its evidence and coverage claim. It does not authenticate evidence,
provide durable replay or prescribe scheduler, query, routing or policy behavior.

This narrow profile avoids introducing a general query language or treating a
source frontier as an event-time completeness watermark. Revisit the contract
when qualified source adapters, durable state or an independently scoped alert
policy is available.
