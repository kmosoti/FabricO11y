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

## Bounded raw input primitive

The actual capture adapter needs raw child-output bytes. The existing artifact
store verifies such files but retains only JSON bodies. Reopening a verified
path to parse its content is excluded: the bytes parsed must be the bytes read
and hashed through the owned descriptor. Investigate one explicit pinned-byte
retention mode in the existing store, rather than another file reader.

Keep digest-only streaming, existing JSON behavior, no-follow component lookup,
hash pins and all metadata rechecks. Materialized raw bodies use the existing
4 MiB per-input bound and share the existing 64 MiB aggregate materialization
budget with JSON (or a caller's smaller budget). They do not gain an additional
budget. Repeated references charge once; incompatible pins and a request for a
body not originally retained fail closed without reopening it.

Before integration run new-mode controls for exact body/hash, non-JSON content,
wrong hash, oversize input, conflicting cache use, replacement/write race and
post-read identity change, plus the existing artifact/JSON tests. These finite
controls do not establish raw producer authenticity. The first consumer is the
current native-cutoff capture adapter; no general storage abstraction is added.

## Separate byte coverage from sender binding

The real local capture supplies exact ACK fields but no per-ACK emission time.
Native journals supply those times and sender fields. Investigate extracting
the ordered sequence/digest/cursor coverage kernel from the context-bound join.
Its result must explicitly establish byte coverage only, never sender identity
or a custody verdict. Preserve the separate native binding checks and the
unchanged independent Python witness. Do not invent timestamps, sort inputs,
silently permit disorder, allocate a second ACK vector, or weaken any refusal.

Run all existing finite controls, plus direct byte-only wrong-digest, missing,
cursor and ordering controls and context-bound sender/window refusals. A separate
binding pass is allowed if it retains linear work and constant auxiliary state;
record its cost rather than assuming the composition is free. Multiple-invalid
inputs may report a different first refusal after separation, but every such
input must remain refused. No acceptance case's named cause is changed here.

Before adoption, compare the context-bound wrapper with the prior prototype05
using the same immutable fixtures, seed, Rust 1.99.0, edition2021, opt3, codegen1,
overflow/debug assertions enabled, debug0 and existing 0/10/1000/100000 rows.
Compile both before timing. Use three alternating old/new pairs under one
contained job; preserve each existing three-sample warm benchmark output.
Compare median stream time at100000 and operation counts, excluding input build
and compilation. A wrapper regression exceeding50% rejects this implementation
pending refinement. Timer-scale cases do not support speed claims. This is a
screen on the observed host, not an optimality proof or an end-to-end service
benchmark. Actual caller adaptation remains a separate gate.

## Resources and completion

All jobs use the existing resource launcher and DATA scratch, with fresh strict
95,000,000,000-byte admission and an explicit prospective reservation. Serialize
heavy jobs. Use the existing bounded bootstrap driver, not a new runner. Results
identify the experiment worktree separately from the frozen launcher worktree.

Completion requires measured baseline, a bounded selected change or an evidenced
rejection, representative defect rejection, affected checks and an explicit list
of removed/retained paths. No alpha issue closes from this round, and no release
or tag is authorized by it. Resuming acceptance is a separate owner decision.
