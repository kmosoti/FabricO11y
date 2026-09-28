# ADR-0019: Keep release maturity in tags, not in names

## Status

Accepted on 2026-09-28. The owner decided this in the architecture-foundation milestone instruction.

## Context

The source tree had `src/alpha`, `tools/alpha`, `docs/ALPHA.md`, `docs/ALPHA-PLAN.md`, phase-numbered architecture pages and a long-lived branch named after a release stage. Every one of those would need renaming when maturity changed, although the architecture would not.

## Decision

Release maturity (`alpha`, `beta`, release candidates) is release metadata. It appears only in version tags such as `v0.1.0-alpha.1`, `v0.1.0-beta.1`, `v0.1.0`, and in package version metadata (the Debian version `0.1.0~alpha.1`). It is not a source namespace, module, tool directory, contract name, roadmap name, documentation section, architectural component or long-lived branch name. It is not replaced by another temporary label (`beta`, `experimental`, `next`, `vnext`, `preview`).

Capabilities name things: Spindle, Spool, Delivery, Control, Retained history, Query, Qualification. Milestone branches are named `milestone/<capability>`.

Historical evidence keeps its words. Experiment records named `alpha-phase*`, the review archive under `docs/experiments/benchmarks/data/alpha-review`, ADR-0010's title and past commit messages are not rewritten; current indexes map them to capabilities. Identifiers whose bytes are part of a registered protocol (the `fabric-alpha-v1:` seed prefix, the runner's ownership marker and `target/alpha-*` output convention) are frozen until a new protocol revision changes them, as listed in [qualification](../QUALIFICATION.md#frozen-harness-identifiers).

## Alternatives considered

- Keep the release-stage names until the release. Every rename would then coincide with a release, the worst time to move files.
- Rewrite historical records. Destroys the link between a record, its content hash and the revision it describes.

## Evidence

The migration map in the [milestone record](../milestones/architecture-foundation.md) lists every moved path; the workspace tests and the oracle suites pass after the moves.

## Consequences

A release no longer changes paths. Readers of old records meet old names; the capability ledger and the experiments index translate. New constraint: reviewers reject new release-stage names in source, tools or current documents.

## Validation

Falsified by a release-stage word appearing as a new active namespace in `src`, `crates`, `tools`, `docs` (outside historical records) or a branch name.

## Related

[Roadmap](../ROADMAP.md), [ADR-0017](ADR-0017-name-the-spindle-and-the-strand.md).
