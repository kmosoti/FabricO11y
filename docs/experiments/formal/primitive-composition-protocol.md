# Primitive and composition engineering round

## Scope and decision

Acceptance is owner-paused. The baseline is `f001175ff4006e2f95294458ce36f4f089d3be7d`.
This round reduces the number of independently maintained rules and tests small
deterministic transitions before exercising effects. It does not rewrite the
product, relax acceptance, or replace independent delivery/query/rate oracles.

Investigate two boundaries: native lifecycle ordering and query source metadata.
First inspect and time existing primitives; retain existing mechanisms when a
proposed replacement cannot demonstrate a smaller state space or measured benefit.
No claim of a globally optimal algorithm follows from these finite experiments.

## Fixed invariants

Preserve encoded bytes, source identities, durable ACK semantics, ordering, query
completeness, authorization, process ownership, resource containment and failure
evidence. Product source and harness source remain separate identities. A pure
transition consumes explicit inputs and produces decisions; clocks, files,
processes, networking and observations remain in effect adapters. No new core
dependency or generic orchestration framework is in scope.

## Experiment order and boundaries

1. Run existing pure-core tests and existing tooling primitive controls on the
   pinned baseline. Record cold build separately from warm execution; record
   actual test counts, command, exit and resource receipt. A filtered zero-test
   run is not evidence.
2. Minimize the retained Fedora sequence-116 counterexample into explicit events.
   Compare a small Rust transition/ordered-join candidate with the unchanged
   independent witness on applicable traces, including tail committed during
   stop, wrong identity/revision, ACK digest mismatch and missing ACK. State any
   assumptions about event order explicitly. Synthetic controls are not acceptance.
3. Evaluate the query metadata representation without materializing or cloning
   reused descriptors. Preserve Owned/Reused behavior. Before adoption use the
   existing Scan/Walk differential, corrupt-source completeness and reuse tests.
   A type cleanup alone supplies no throughput claim.
4. Adopt only a vertical slice that replaces existing duplicated logic, with its
   affected integration controls. Do not retain an unused parallel framework.

For any algorithm comparison fix the same input, seed, result semantics and build
profile before execution. Measure work as input elements examined, auxiliary
memory as retained entries, and elapsed time independently. Include parsing and
I/O only when explicitly named. Keep unfavorable and failed results. Prefer
linear streaming joins when ordering is established; otherwise charge the cost
of sorting or indexing instead of assuming ordered input.

## Resources and completion

All jobs use the existing resource launcher and DATA scratch, with fresh strict
95,000,000,000-byte admission and an explicit prospective reservation. Serialize
heavy jobs. Use the existing bounded bootstrap driver, not a new runner. Results
identify the experiment worktree separately from the frozen launcher worktree.

Completion requires measured baseline, a bounded selected change or an evidenced
rejection, representative defect rejection, affected checks and an explicit list
of removed/retained paths. No alpha issue closes from this round, and no release
or tag is authorized by it. Resuming acceptance is a separate owner decision.
