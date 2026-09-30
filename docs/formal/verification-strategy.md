# Verification strategy

Candidate generation may be stochastic; acceptance is reproducible verification ([ADR-0018](../decisions/ADR-0018-accept-work-on-executable-evidence.md)). This page says which technique answers which question. The techniques address different uncertainties; they are not a ladder in which a formal model makes the checks below it irrelevant. The [verification matrix](verification-matrix.md) maps each product claim to the checks that exercise it today, and the [check registry](../../xtask/checks.json) lists the commands.

| Question | Technique | In this repository |
| --- | --- | --- |
| Is code allowed to depend on this layer? | Cargo-metadata layer gate | `cargo xtask check-layers`, [ADR-0015](../decisions/ADR-0015-adopt-a-hexagonal-architecture.md), fixtures in [xtask/tests/gates.rs](../../xtask/tests/gates.rs) |
| Can semantic core code obtain hidden ambient inputs or perform effects? | `no_std`, dependency allowlist, forbid-unsafe, Clippy restrictions, purity gate | `cargo xtask check-core-purity`, [ADR-0016](../decisions/ADR-0016-keep-a-pure-semantic-core.md), [no_std experiment](../experiments/formal/core-no-std.md) |
| Can invalid or prohibited states be expressed through the supported API? | Private constructors, validated newtypes | `StrandId` (no generation 0), `IncomingBatch.sequence: NonZeroU64`; compile-fail tests are not yet used |
| Does a small finite decision algebra handle every case? | Exhaustive truth tables | ADR-0013 table in [`fabric_core::delivery`](../../crates/fabric-core/src/delivery.rs) |
| Does optimized or production logic agree with an independent simple implementation? | Differential testing | the delivery kernel against a frozen transcription of the base loop ([crates/fabric-app/tests/delivery.rs](../../crates/fabric-app/tests/delivery.rs)); server queries against the Python [query oracle](../../tools/qualification/QUERY_ORACLE.md) |
| Do algebraic laws hold over large generated spaces? | Property testing (proptest) | [fabric-properties](../../crates/fabric-properties/src/lib.rs): 12 properties of the kernels, each stated from an ADR or view, 2,000 cases each (`kernel-properties`) |
| Does a pure kernel's contract hold for every value of its input types? | Bounded model checking (Kani) | [proofs.rs](../../crates/fabric-core/src/proofs.rs): 11 harnesses under `cfg(kani)`, also checking panics and overflow (`kani-core`) |
| Can untrusted bytes panic a decoder or break recovery? | Coverage-guided fuzzing (cargo-fuzz, libFuzzer) | [fuzz/](../../fuzz/): batch decoding, query requests, frame-log recovery (`fuzz-smoke`); committed inputs replayed on stable (`fuzz-corpus`) |
| Does delivery keep its promises when the network drops, delays or partitions? | Network simulation (turmoil) | [fabric-sim](../../crates/fabric-sim/tests/delivery.rs): the real router and store behind a simulated network (`network-simulation`) |
| Is a dependency vulnerable, yanked, badly licensed or from an unknown source? | Dependency policy (cargo-deny) | [deny.toml](../../deny.toml) (`dependency-policy`) |
| Which code did the tests execute? | Source-based coverage (cargo-llvm-cov) | `coverage-report`, a report and never a gate: coverage shows what ran, not what was asserted |
| Does behavior stay equivalent under meaning-preserving transformations? | Metamorphic testing | journal-only versus sealed-Segment answers compared directly (`journal_and_segment_representations_answer_identically`); other relations below |
| Would the tests notice plausible incorrect logic? | Mutation testing | [semantic mutants](../../xtask/mutants.json) (`cargo xtask mutants`), oracle mutation controls, `cargo-mutants` calibration on core and app |
| Do effects and recovery behave correctly under real failures? | Fault injection | frame-log sync-failure and process-death seams, [delivery fault harness](../../tools/qualification/delivery_faults.py), kill probe |
| Can the modeled state machine violate its invariant within the explored model? | TLA+ model checking | [delivery ownership](../../formal/delivery/README.md), [transport credit](../../formal/transport/README.md) |
| Did a real run do something the model forbids? | Trace validation with TLC | [trace_check.py](../../formal/delivery/trace_check.py) maps fault-run transcripts onto `DeliveryOwnership` actions |
| Does the complete system satisfy the target operating profile? | Registered qualification | [qualification](../QUALIFICATION.md) |

## Oracle independence

The Python delivery, query and rate oracles were frozen before the Rust code they grade and are kept in a different language on purpose. They are not rewritten in Rust for uniformity. A test generated alongside an implementation is not an independent oracle, and changing an oracle, an expected negative-control outcome, a formal invariant or a registered protocol is a trust-boundary change made in its own commit.

## Metamorphic relations

The matrix records which relations are checked today.

| Relation | Statement | Current check |
| --- | --- | --- |
| Journal/Segment equivalence | For the same retained committed history, `Query(Journal) = Query(Segments)` | direct: `journal_and_segment_representations_answer_identically` answers log, filtered log, metric and rate queries from the journal alone, then again after a restart seals that journal into a Segment; rows, completeness, gaps and freshness must be equal. Mutant `M-HIST-SEGMENT` checks that the test can fail |
| Seal invariance | If retention did not change, `Query(before seal) = Query(after seal)` | the same test |
| Replay invariance | Durable observable state before restart equals state after replay, within the recovery semantics | delivery and control end-to-end restart tests; fault harness |
| Duplicate retry invariance | One durable commit plus any number of identical retransmissions of the same Strand sequence yields one logical Batch | kernel truth table, differential test, delivery oracle |
| Pagination composition | If all pages stay retained, `concat(Page_1..Page_n) = FullQuery` | history tests against the oracle |
| Storage-representation invariance | Journal only, Segments only, and journal plus Segments give equal semantic answers | journal only versus Segments only: the same test; mixed journal and Segments: the oracle-graded history tests |
| Irrelevant-environment invariance | Core functions do not change when irrelevant process environment changes | structural: `no_std` leaves the core no way to read it |

## Mutation policy

Semantic mutants in [xtask/mutants.json](../../xtask/mutants.json) each name the contract they threaten, the incorrect behavior, the file and exact replacement, the checker command and the test that must fail. `cargo xtask mutants` applies them to a copy of the tree, first confirming each checker passes unmutated, and classifies each as caught, caught elsewhere, survived, stale or inconclusive. The oracle suites carry their own mutation controls, and the delivery fault harness has `--mutate drop-recovered`.

`cargo-mutants` is calibrated narrowly on pure or nearly pure decision code (`fabric-core`, `fabric-app`), never on the filesystem or network stack in routine CI. Results are classified as caught, unviable, equivalent, excluded with justification, survived or inconclusive. There is no mutation-score threshold. The rule is: no unexplained surviving mutation may alter a claimed safety or semantic contract.

Calibration in the architecture-foundation milestone (`cargo-mutants` 27.1.0, `cargo mutants -p fabric-core -p fabric-app --test-package fabric-core --test-package fabric-app`): 34 mutants, 27 caught, 6 unviable (they return `Default::default()` for types without `Default`, or `Box` in the `no_std` crate), 1 missed. The missed mutant replaces `sequence < last` with `sequence <= last` in `decide_delivery`; it is **equivalent**, because the preceding branch already handles `sequence == last`. With only the core's own tests: 29 mutants, 23 caught, 5 unviable, the same 1 equivalent. Outcome lists are kept under [data/architecture-foundation/cargo-mutants](../experiments/benchmarks/data/architecture-foundation/cargo-mutants/missed.txt).

Since the verification-foundation milestone, `cargo xtask cargo-mutants` runs that command and fails on any missed mutant not listed with a reason in [cargo-mutants-equivalent.json](../../xtask/cargo-mutants-equivalent.json), and on a listed entry that no longer survives. It runs in the extended profile. Survivors are named with their file position, so two mutants with the same description stay distinct and an equivalence argument must be renewed when its code moves. Timeouts count as detected, because tests that never finish fail CI; the limitation is that a genuinely surviving mutant on an overloaded host could be misread as a timeout, so the audit runs without competing load and prints every timeout.

Every checker added in the verification-tooling milestone has its own mutants: property tests (`M-PROP-*`), Kani harnesses through [formal/kani/check.sh](../../formal/kani/check.sh), which prints libtest-style lines (`M-KANI-*`), the fuzz corpus replay (`M-FUZZ-IDENTIFY`) and the network simulation (`M-SIM-DUPLICATE`).

## Verification layer

Property tests, fuzz target bodies and simulations live in crates of the `verification` layer ([layers.json](../architecture/layers.json)): they may depend on any product layer, and no product layer may depend on them. Test-only dependencies (proptest, turmoil, hyper clients) therefore never reach product code, and the core keeps no dependencies; its Kani harnesses compile only under `cfg(kani)`. Kani's function-contract attributes and autoharness are not used while they are unstable.

## Counterexamples become fixtures

Every material defect found by an oracle, mutant, property test, fuzzer, model checker, fault harness, review or incident is minimized where practical and kept as a deterministic regression fixture recording origin, original seed or trace, minimal reproducer, contract violated and fix commit. Random seeds alone are not enough; important minimized cases become named tests. The registry is [counterexamples.json](counterexamples.json); `cargo xtask check-counterexamples` (fast profile) fails if an entry lacks a field, if its reproducer test no longer exists in its file, or if its fix commit is unknown.

## Formal models and their limits

Delivery fault-run transcripts are validated against the unchanged `DeliveryOwnership` model by [trace_check.py](../../formal/delivery/trace_check.py). It generates a module that `INSTANCE`s the model and maps each observation to model actions:
- an attempt is `Receive`;
- an ACK through N is `Acknowledge` of every sourced identity up to N;
- a Spool observation that no longer retains an identity is `Forget`;
- the recovered set must equal `durable` at the end.

`Commit` and `ReceiverCrash` are hidden steps allowed between observations. TLC searches for a model behavior that reaches the end of the trace while the model's `AckSafety` and `RetainedOrDurable` hold; the invariant `TraceNotAccepted` is violated exactly when such a behavior exists. A trace with no such behavior is rejected.

Its controls ([test_trace_check.py](../../formal/delivery/test_trace_check.py)) accept a real server-kill transcript and a legal forget, and reject an early forget, an ACKed identity missing after recovery, and a resend after forget. The extended `delivery-faults` check validates every fault-run transcript this way. The model has no bytes, sequences or credentials, which the delivery oracle checks, and no node crash action, because it assumes the upstream copy survives. Limits stay explicit: TLA+ does not prove the Rust code; Rust tests do not prove fsync hardware behavior; the delivery model does not prove query correctness; the query oracle does not prove retention hardware reliability.

## Verification receipts

`cargo xtask checks --profile <fast|extended|qualification>` runs the registry and writes one JSON receipt per check under `target/verification/receipts/`: check and property IDs, candidate commit and whether the tree was dirty, the last commits touching the specification and oracle files, toolchain versions, command, profile, fixtures and bounds, negative controls, result, exit code, duration, SHA-256 of the output, and unchecked behavior. A receipt proves only that the record has this structure; it cannot prove the command ran. Receipts produced by the runner or CI are preferred over prose; none are signed in this milestone. A required tool or path that is missing yields `environment-unavailable` and an overall INCOMPLETE status (exit 3), never success.

## CI

The required fast path is the `fast` profile: formatting, locked compilation, workspace tests, Clippy on every crate, the layer and purity gates, the kernel properties, the fuzz-corpus replay, the network simulation, the qualification-runner, workload/rate, delivery-oracle and query-oracle suites, and documentation integrity. Qualification workloads, soak, fleet scale, privileged installation, the full mutation suite and large model checks are never added to every pull request; they belong to targeted qualification, scheduled checks or explicit release-readiness runs. The extended profile adds Kani, time-boxed fuzzing, the dependency policy and a coverage report; it runs weekly, on demand and on pull requests that touch what it verifies.
