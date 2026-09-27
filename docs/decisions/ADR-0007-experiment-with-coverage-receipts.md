# ADR-0007: Experiment with authenticated block coverage

## Status

Experimental for the separate storage research package. This selects a correctness
prototype, not an application query API, disk format or production trust channel.

## Context

[S1](../experiments/ablation/storage-query-s1-run-01.md) owns all rows in one immutable
snapshot. Its no-false-negative argument assumes that the block list is complete.
Once data availability is modeled separately, an empty result cannot distinguish
absence of matches from omitted blocks without additional evidence.

## Decision

In the [coverage experiment](../experiments/ablation/coverage-e1-protocol.md), a trusted
builder validates conservative summaries against owned rows and authenticates
ordered block commitments. A caller retains the snapshot anchor independently of
the receipt. Each receipt accounts for every block with `Scanned`, `Excluded`, or
`Unavailable`. A metadata-only verifier returns complete, incomplete, or an error.

Use exact ordered sets for tenant and Log-token summaries in this first experiment.
This makes the inclusion rule inspectable without adding probabilistic filter
behavior to the trust-boundary test. It does not replace S1's Bloom summaries or
establish an economical indexing strategy. SHA-256 is a research-package dependency;
CRC32 in the application log remains unchanged.

## Alternatives and correctness argument

- **Trust the iterated list:** omitting the only matching block can silently produce
  a complete empty answer. A fixed external root/count detects this omission.
- **Authenticate summaries without validation:** a valid commitment to an incorrect
  summary still permits a false-negative exclusion. Validate inclusion before sealing;
  preserve a faulty-builder negative control to expose this limitation.
- **Require raw rows at verification:** permits independent full-scan checking, but
  answers a different question from metadata coverage. Retain that full scan as the
  test oracle, outside the receipt verifier.

Conditional on a correct builder, trusted anchor, collision-resistant hashing and
an exact scan executor, partition the snapshot by its authenticated block ranges.
Every verified block is scanned exactly, safely excluded by its valid summary, or
explicitly unavailable. The last case prevents completeness. This is a conditional
argument; a scan marker alone does not prove execution, and hashes cannot establish
summary semantics or freshness of an anchor supplied by an untrusted peer.

## Consequences and validation

Coverage can be examined without reading rows, but it requires a trusted source of
snapshot identity and a validated builder. Metadata size, duplicate work, proof
construction cost, anchor retention and publication durability remain unmeasured.
The prototype does not persist anchors, model physical loss, or implement resumable
answers. The [registered corpus and fault cases](../experiments/ablation/coverage-e1-protocol.md)
must falsify omitted blocks, substituted metadata, invalid summaries, malformed
proofs and mistaken completeness. Independent mutation checks must fail when root
binding or summary validation is removed.

## Related

- [Current system](../architecture/system.md)
- [Framed local-log decision](ADR-0006-use-framed-local-log.md)
- [Prototype API contract](../../tools/storage-probe/COVERAGE_API.md)

The [first E1R run](../experiments/ablation/coverage-e1-run-01.md) passed the finite corpus and
independent reviews, with failing root-binding and builder-validation mutations. This supports
continued research under the assumptions; it does not change this decision to production acceptance.
