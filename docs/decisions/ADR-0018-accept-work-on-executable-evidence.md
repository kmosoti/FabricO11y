# ADR-0018: Accept work on executable evidence, not on model review

## Status

Accepted on 2026-09-28 under the owner's architecture-foundation authorization. It supersedes the cross-family review requirements in the retired release-stage plan and in the previous `AGENTS.md`.

## Context

Completion criteria required approval by language models from different families. When one model's quota ran out, phase promotion stopped even though every executable check had passed; when a reviewer ran without shell access, its verdict could not be reproduced. Those reviews did find real defects, and the repository kept their counterexamples as regressions. The durable value was the counterexample and the check, not the verdict.

Generated code and generated tests can share blind spots, so a test written alongside an implementation is not independent evidence (see the [research digest](../research/generator-verifier.md)).

## Decision

Candidate generation may be stochastic; acceptance is reproducible verification.

- Completion is judged against the [product contract](../PRODUCT-CONTRACT.md), executable checks, negative controls, formal-model results where relevant, fault injection, registered measurements where relevant, and explicit limitations. No language-model approval, consensus, quota or availability is a gate, and no unnamed AI reviewer replaces one.
- **Independent oracles** stay independent. The Python delivery, query and rate oracles are not rewritten in Rust for uniformity; their separate implementation is the point. A test generated with an implementation does not count as an independent oracle.
- **Trust-boundary changes** are explicit: a change to the product contract, an oracle, a negative control's expected outcome, a formal invariant, a registered protocol or verification policy goes in its own commit with its reason, never mixed into an implementation change.
- **Negative controls**: every checker ships with a representative defect it must reject (semantic mutants, fixture workspaces, injected faults).
- **Counterexamples** found by any means (oracle, mutation, property test, fuzzer, model checker, fault harness, review, incident) are minimized where practical and kept as deterministic regression fixtures with origin, original seed or trace, minimal reproducer, contract violated and fix commit.
- **Verification receipts**: important checks are run through the [check registry](../../xtask/checks.json), which records command, revision, toolchain, result and output hash. A receipt proves the record's structure, not that the command ran; receipts emitted by the runner or CI are preferred over prose, and none are signed in this milestone.
- **Claims**: "passed", "proved" and "qualified" require the corresponding check to have run and support that exact claim; "not run", "interrupted", "failed", "inconclusive" and "environment unavailable" are reported as such.
- Historical review records stay as historical evidence; they are not continuing gates.

## Alternatives considered

- Keep cross-family review as a gate. Blocks on SaaS availability and certifies prose, not behavior.
- Replace it with a single mandatory AI reviewer. Same failure mode with one fewer check.
- Drop review entirely without strengthening checks. Loses the defects reviews used to find; hence negative controls, mutants and counterexample fixtures become required.

## Evidence

Past reviews' findings survive as tests (for example the Unicode gap cap, interrupted-append recovery and the journal length checksum). This milestone ran the existing oracle mutation controls and the semantic-mutant registry; results are in the [milestone record](../milestones/architecture-foundation.md).

## Consequences

Easier: completion no longer depends on external model availability; evidence is reproducible. Harder: every checker needs its own negative control, and oracle changes need explicit justification. Review by people or models remains welcome as a way to find defects; its output enters the repository as a counterexample or a check.

## Validation

Falsified if a change is accepted whose only support is a model's verdict, or if an oracle or expected negative-control outcome changes inside an ordinary implementation commit.

## Related

[Verification strategy](../formal/verification-strategy.md), [verification matrix](../formal/verification-matrix.md), [ADR-0005](ADR-0005-ack-after-durable-commit.md).
