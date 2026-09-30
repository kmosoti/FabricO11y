# ADR-0021: Add property, bounded-model, fuzz and network-simulation checks

## Status

Accepted on 2026-09-30 in the verification-tooling milestone. The owner asked for this toolset after a review of the Rust verification ecosystem.

## Context

Before this milestone, every contract rested on four kinds of check:

- example-based and differential tests;
- independent Python oracles;
- hand-written semantic mutants and a cargo-mutants audit;
- TLA+ models with trace validation.

Four questions had no direct check:

- Do the kernels' contracts hold beyond hand-picked inputs, over whole input ranges?
- Can untrusted bytes panic a decoder or break journal recovery?
- Does delivery keep its promises when the network, rather than a process, fails?
- Are the dependencies vulnerable or badly licensed?

## Decision

Adopt six tools, each behind a registered check with its own negative control.

- **Property tests (proptest 1.11).** They run in `fabric-properties`. Each property states a kernel contract independently of its code.
- **Bounded model checking (Kani 0.68).** Plain `#[kani::proof]` harnesses live in `fabric-core` under `cfg(kani)`. Function contracts, autoharness and other unstable Kani features are not used.
- **Coverage-guided fuzzing (cargo-fuzz 0.13 and libFuzzer).** The targets are the decoders of untrusted bytes and recovery. The target bodies live in `fabric-fuzz-targets`, and a stable test replays every committed corpus and regression input.
- **Network simulation (turmoil 0.7).** It runs in `fabric-sim`, driving the real server router and store over a simulated network.
- **Dependency policy (cargo-deny 0.20).** It is configured in `deny.toml`. cargo-audit is not added, because cargo-deny reads the same RustSec database.
- **Coverage report (cargo-llvm-cov 0.9).** It is a report, never a gate.

Clippy with warnings denied now covers every workspace crate, not only the inner layers.

A new `verification` layer holds the test-only crates. Its crates may depend on any product layer, and no product layer may depend on them. This keeps proptest, turmoil and HTTP clients out of product code and leaves the core's zero-dependency policy unchanged.

## Alternatives considered

| Alternative | Why it was not taken |
| --- | --- |
| proptest as a dev-dependency of `fabric-core` | This would widen the core-purity allowlist. A verification-layer crate gives the same coverage without a policy change. |
| Loom or Shuttle for concurrency | Both require replacing the standard sync types throughout the server. Loom's last release was in April 2024. Protocol-level interleavings are already covered by TLA+ and trace validation. |
| Miri | The only unsafe code is libc calls (`statvfs`, `sysconf`, a signal handler) and a research allocator, and Miri cannot execute most foreign calls. |
| Creusot or Verus | Both call themselves experimental. Bounded Kani proofs of a small, pure core cover the same kernels at a much lower cost. |
| A coverage threshold | Coverage measures execution, not assertion. A threshold invites tests that run code without checking it. |

## Evidence

The results are recorded in the [milestone record](../milestones/verification-tooling.md):

- Kani found two defects in the core, now fixed and registered as counterexamples:
  - retention under-counted saturated byte totals (`CX-RETENTION-SATURATED-TOTAL`);
  - rates were NaN or infinite on non-reset rows (`CX-RATE-NON-FINITE`).
- Every new checker's mutants are caught.
- Skipping batch validation made the fuzzer crash at once on the empty input.

## Consequences

- **Test time.** Every pull request runs about 30 s more tests. The extended job takes about 20 minutes longer.
- **Toolchains.** Fuzzing needs a nightly toolchain, and Kani installs its own. Missing tools make the extended profile report INCOMPLETE (exit 3), never success.
- **Limits.** Kani results hold within each harness's unwinding bound. A fuzz run finding no crash is evidence, not proof of absence. The simulation leaves process crashes to the real-process fault harness, and because the commit thread runs on a real OS thread, its runs are not fully deterministic.

## Validation

This decision is falsified by any of these:

- a new checker's registered mutant surviving;
- a product crate depending on a verification-layer crate without `check-layers` failing;
- a dependency with a licence outside `deny.toml` entering without `dependency-policy` failing.

## Related

- [ADR-0015](ADR-0015-adopt-a-hexagonal-architecture.md) (layers)
- [ADR-0016](ADR-0016-keep-a-pure-semantic-core.md) (purity)
- [ADR-0018](ADR-0018-accept-work-on-executable-evidence.md) (executable evidence)
- [Verification strategy](../formal/verification-strategy.md)
