# Frozen experiment snapshots

Use a bounded in-memory contract in `fabric-evidence` with a fixed nonempty
roster, named reporting cutoff, evaluator and configuration digest claims,
unit and aggregation. Each roster member has exactly one typed known or
unknown outcome. A supplementary snapshot names its parent and a distinct
cutoff; validation checks the same population and measurement definition.
Cutoff names do not imply a wall-clock time or ordering.

Comparisons fail on population or definition changes. They retain exactness for
known values, expose unknown outcomes from both sides, and compute numeric
differences in a width that covers all pairs of signed 64-bit values. They do
not infer significance or causality. Missing cases cannot silently disappear
from the denominator, though explicitly unknown cases may become known later.

The type is immutable after validated construction. It is not a wire schema,
receipt, durable record, authorization decision, corpus cluster or review input
binding. Callers must independently bound decoding and reject duplicate keys
before constructing the ordered roster and outcome maps. Exact cutoff and
identity persistence and authorized parent readback remain separate work.

The separate reference-only `fabric.snapshot.v1` application payload supports at
most 16 ordered cases and must also fit the existing 4 KiB application payload
and shape limits. A known case names a same-owner report UUID, expected
canonical application-payload digest and optional nonzero source ID; an unknown
case retains its reason. These are claims, not receipts or authenticated reads.
The parent report ID comes only from the validated application envelope; the
payload binds its expected canonical digest. Supplement checking requires the
same scope, population and definition, and a strictly increasing ordinal on
the same immutable cutoff source. Provenanced timestamps are retained but are
not compared to establish ordering. The pure matcher cannot authenticate a
result or establish a receipt; adapters must do so before using the match.

The batch adapter can verify a snapshot through the owner's configured
canonical reader. It authenticates the root and every known case, and, for a
supplement, the directly named parent and its known cases, before returning a
bounded summary. Parent chains beyond one boundary are refused. Each lookup
uses the exact outbox receipt and one shared deadline; synchronous local disk
operations are checked around calls but cannot be interrupted midway. An
unknown case stays unknown. These checks do not establish corpus or review
bindings or analytical projection visibility.

Two explicitly referenced boundaries may also be compared without assuming a
parent relation. A `fabric.snapshot.case-measurement.v1` case result binds its
case identifier and full measurement definition to an i64 value and exactness.
The adapter verifies both roots and all known case references under one deadline,
then applies the pure comparison to the fixed roster. Unrecognized schemas are
unavailable; malformed recognized measurements fail closed. The output uses a
decimal string for the i128 difference. These checks do not establish corpus or
review bindings or causal significance.
