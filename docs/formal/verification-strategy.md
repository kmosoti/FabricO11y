# Verification strategy

Candidate generation may be stochastic; acceptance is reproducible verification ([ADR-0018](../decisions/ADR-0018-accept-work-on-executable-evidence.md)). This page says which technique answers which question. The techniques address different uncertainties; they are not a ladder in which a formal model makes the checks below it irrelevant. The [verification matrix](verification-matrix.md) maps each product claim to the checks that exercise it today, and the [check registry](../../xtask/checks.json) lists the commands.

| Question | Technique | In this repository |
| --- | --- | --- |
| Is code allowed to depend on this layer? | Cargo-metadata layer gate | `cargo xtask check-layers`, [ADR-0015](../decisions/ADR-0015-adopt-a-hexagonal-architecture.md), fixtures in [xtask/tests/gates.rs](../../xtask/tests/gates.rs) |
| Can semantic core code obtain hidden ambient inputs or perform effects? | `no_std`, dependency allowlist, forbid-unsafe, Clippy restrictions, purity gate | `cargo xtask check-core-purity`, [ADR-0016](../decisions/ADR-0016-keep-a-pure-semantic-core.md), [no_std experiment](../experiments/formal/core-no-std.md) |
| Can invalid or prohibited states be expressed through the supported API? | Private constructors, validated newtypes | `StrandId` (no generation 0), `IncomingBatch.sequence: NonZeroU64`; compile-fail tests are not yet used |
| Does a small finite decision algebra handle every case? | Exhaustive truth tables | ADR-0013 table in [`fabric_core::delivery`](../../crates/fabric-core/src/delivery.rs) |
| Does optimized or production logic agree with an independent simple implementation? | Differential testing | the delivery kernel against a frozen transcription of the base loop ([crates/fabric-app/tests/delivery.rs](../../crates/fabric-app/tests/delivery.rs)); server queries against the Python [query oracle](../../tools/qualification/QUERY_ORACLE.md) |
| Do algebraic laws hold over large generated spaces? | Property testing | not yet adopted; the exhaustive small-domain differential covers the delivery kernel |
| Does behavior stay equivalent under meaning-preserving transformations? | Metamorphic testing | partially: oracle-graded answers across sealed and unsealed storage; the explicit relations below are planned |
| Would the tests notice plausible incorrect logic? | Mutation testing | [semantic mutants](../../xtask/mutants.json) (`cargo xtask mutants`), oracle mutation controls, `cargo-mutants` calibration on core and app |
| Do effects and recovery behave correctly under real failures? | Fault injection | frame-log sync-failure and process-death seams, [delivery fault harness](../../tools/qualification/delivery_faults.py), kill probe |
| Can the modeled state machine violate its invariant within the explored model? | TLA+ model checking | [delivery ownership](../../formal/delivery/README.md), [transport credit](../../formal/transport/README.md) |
| Does the complete system satisfy the target operating profile? | Registered qualification | [qualification](../QUALIFICATION.md) |

## Oracle independence

The Python delivery, query and rate oracles were frozen before the Rust code they grade and are kept in a different language on purpose. They are not rewritten in Rust for uniformity. A test generated alongside an implementation is not an independent oracle, and changing an oracle, an expected negative-control outcome, a formal invariant or a registered protocol is a trust-boundary change made in its own commit.

## Metamorphic relations

These relations are part of the plan; the matrix records which are checked today.

| Relation | Statement | Current check |
| --- | --- | --- |
| Journal/Segment equivalence | For the same retained committed history, `Query(Journal) = Query(Segments)` | indirect: oracle-graded answers across sealed and unsealed data; no direct comparison |
| Seal invariance | If retention did not change, `Query(before seal) = Query(after seal)` | not checked directly |
| Replay invariance | Durable observable state before restart equals state after replay, within the recovery semantics | delivery and control end-to-end restart tests; fault harness |
| Duplicate retry invariance | One durable commit plus any number of identical retransmissions of the same Strand sequence yields one logical Batch | kernel truth table, differential test, delivery oracle |
| Pagination composition | If all pages stay retained, `concat(Page_1..Page_n) = FullQuery` | history tests against the oracle |
| Storage-representation invariance | Journal only, Segments only, and journal plus Segments give equal semantic answers | not checked directly |
| Irrelevant-environment invariance | Core functions do not change when irrelevant process environment changes | structural: `no_std` leaves the core no way to read it |

## Mutation policy

Semantic mutants in [xtask/mutants.json](../../xtask/mutants.json) each name the contract they threaten, the incorrect behavior, the file and exact replacement, the checker command and the test that must fail. `cargo xtask mutants` applies them to a copy of the tree, first confirming each checker passes unmutated, and classifies each as caught, caught elsewhere, survived, stale or inconclusive. The oracle suites carry their own mutation controls, and the delivery fault harness has `--mutate drop-recovered`.

`cargo-mutants` is calibrated narrowly on pure or nearly pure decision code (`fabric-core`, `fabric-app`), never on the filesystem or network stack in routine CI. Results are classified as caught, unviable, equivalent, excluded with justification, survived or inconclusive. There is no mutation-score threshold. The rule is: no unexplained surviving mutation may alter a claimed safety or semantic contract.

## Counterexamples become fixtures

Every material defect found by an oracle, mutant, property test, fuzzer, model checker, fault harness, review or incident is minimized where practical and kept as a deterministic regression fixture recording origin, original seed or trace, minimal reproducer, contract violated and fix commit. Random seeds alone are not enough; important minimized cases become named tests. Examples kept so far: the journal length checksum, the Unicode gap cap, interrupted-append recovery, the oversize-line and FIFO reader cases, and the `u64::MAX` Strand overflow found by the delivery extraction.

## Formal models and their limits

The long-term aim for the delivery model is: implementation trace, abstracted to model actions (`Receive`, `Commit`, `Acknowledge`, `Forget`, `Retry`, `Crash`), checked for a legal modeled transition. The fault harness transcript already records attempts, responses, Spool state and recovered records; mapping those records to model actions is verification-foundation work and not implemented. Limits stay explicit: TLA+ does not prove the Rust code; Rust tests do not prove fsync hardware behavior; the delivery model does not prove query correctness; the query oracle does not prove retention hardware reliability.

## Verification receipts

`cargo xtask checks --profile <fast|extended|qualification>` runs the registry and writes one JSON receipt per check under `target/verification/receipts/`: check and property IDs, candidate commit and whether the tree was dirty, the last commits touching the specification and oracle files, toolchain versions, command, profile, fixtures and bounds, negative controls, result, exit code, duration, SHA-256 of the output, and unchecked behavior. A receipt proves only that the record has this structure; it cannot prove the command ran. Receipts produced by the runner or CI are preferred over prose; none are signed in this milestone. A required tool or path that is missing yields `environment-unavailable` and an overall INCOMPLETE status (exit 3), never success.

## CI

The required fast path is the `fast` profile: formatting, locked compilation, workspace tests, Clippy on the inner layers, the layer and purity gates, the qualification-runner, workload/rate, delivery-oracle and query-oracle suites, and documentation integrity. Qualification workloads, soak, fleet scale, privileged installation, the full mutation suite and large model checks are never added to every pull request; they belong to targeted qualification, scheduled checks or explicit release-readiness runs.
