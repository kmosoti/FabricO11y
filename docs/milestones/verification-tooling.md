# Milestone: verification tooling

Status: complete pending CI and merge. Base: `main` at `e68d6ce`.

This milestone adopts property testing, bounded model checking, coverage-guided fuzzing, network simulation, a dependency policy and a coverage report, as [ADR-0021](../decisions/ADR-0021-add-property-model-fuzz-and-simulation-checks.md) records. It also extends Clippy to every crate. Kani found two defects in the core, both fixed here.

The milestone changes no wire format, persisted format, oracle rule or protocol. Its one product-contract clarification is how a non-finite rate is reported.

## Acceptance criteria

| ID | Criterion | Deciding evidence |
| --- | --- | --- |
| VT-1 | Clippy with warnings denied passes on every crate and target | the `clippy` check |
| VT-2 | The kernels have property tests stated from their contracts, each shown able to fail | `kernel-properties`; mutants M-PROP-GROUP, M-PROP-PAGE, M-PROP-TEXT, M-PROP-RET-BYTES |
| VT-3 | The core has Kani proofs with no new dependency, each shown able to fail | `kani-core`; `check-core-purity` unchanged; mutants M-KANI-SEQ, M-KANI-DUP, M-KANI-RET-SATURATE |
| VT-4 | Decoders of untrusted bytes and journal recovery are fuzzed, and committed inputs replay on stable | `fuzz-smoke`, `fuzz-corpus`; mutant M-FUZZ-IDENTIFY |
| VT-5 | Delivery is simulated over a faulty network against the real server | `network-simulation`; mutant M-SIM-DUPLICATE |
| VT-6 | Dependencies are checked for advisories, licences and sources | `dependency-policy`, with a failing control |
| VT-7 | Coverage is reported, not gated | `coverage-report` |
| VT-8 | Test-only crates cannot reach product code | the `verification` layer in `layers.json`, with a failing control |
| VT-9 | Defects found by the new tools are fixed and kept | counterexamples registered with fix commits and regression tests |
| VT-10 | The records agree, and the contributor guide describes the tools once CI shows them stable | the ADR, verification strategy, matrix, system view and layer diagram, roadmap, current state, and CONTRIBUTING; docs check exit 0 |

## Definition of done

VT-1 to VT-10 are met, with the commands and exits recorded below. The PR's CI is green (Rust, Documentation, Extended verification), and the PR is merged into `main`.

## Results

Environment: a 4-CPU container with rustc 1.94.1, 1.98.0 (CI) and nightly. Tools: proptest 1.11.0, Kani 0.68.0, cargo-fuzz 0.13.2, turmoil 0.7.2, cargo-deny 0.20.2 and cargo-llvm-cov 0.9.1.

| Criterion | Command | Exit | Result |
| --- | --- | --- | --- |
| VT-1 | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 0 | Two findings fixed first: a unit `let` in the FOL2 log, and an eight-argument research example (allowed with its reason) |
| VT-2 | `cargo test -p fabric-properties` | 0 | 12 properties, 2,000 cases each |
| VT-2 | `cargo xtask mutants --only M-PROP-…` | 0 | All four caught. M-PROP-PAGE was first *caught elsewhere*: the paging property looped forever on the mutant. The property now bounds its pages and fails cleanly |
| VT-3 | `bash formal/kani/check.sh`, first run | 1 | **2 of 11 failed**, both genuine: see VT-9 |
| VT-3 | the same, after the fixes | 0 | 11 of 11 verified (about 90 s) |
| VT-3 | `cargo xtask mutants --only M-KANI-…` | 0 | All three caught |
| VT-4 | `bash fuzz/smoke.sh 60`, and 120 s per target beforehand | 0 | No failure. 120 s gave 9.9 M, 3.8 M and 221 k executions for batch decoding, query requests and frame recovery |
| VT-4 | fuzzing with batch validation skipped | 1 | libFuzzer crashed at once on the empty input (the node-id `unwrap`). The input is kept as `fuzz/regressions/batch_identify/empty-input`, and M-FUZZ-IDENTIFY is caught by the stable replay |
| VT-5 | `cargo test -p fabric-sim` | 0 | Two scenarios, 3 seeds each: lossy links; held answers and partitions. Each asserts that attempts went unanswered, and that the history read back is sequences 1 to 40, once each. A first fault schedule starved the sender (a 3 s cycle against a 5 s attempt timeout): a harness defect, since fixed |
| VT-5 | `cargo xtask mutants --only M-SIM-DUPLICATE` | 0 | Caught |
| VT-6 | `cargo deny --locked check` | 0 | Advisories, bans, licences and sources all ok; 3 duplicate versions reported |
| VT-6 | the same policy without ISC | 4 | `ring` and its dependants rejected |
| VT-7 | `cargo llvm-cov --workspace --locked --summary-only` | 0 | 81.97 % of lines and 79.46 % of regions. Binaries started as subprocesses are not instrumented |
| VT-8 | `cargo xtask check-layers` with `fabric-properties` added as a dependency of `fabric-app` | 1 | `LAYER_FORBIDDEN_EDGE fabric-app (app) -> fabric-properties (verification)` |
| VT-9 | `cargo test -p fabric-core` | 0 | Both regression tests fail on the pre-fix code and pass on the fix |
| VT-1 to VT-9 | `cargo xtask checks --profile fast` | 0 | 20 of 20 |
| VT-3 to VT-7 | `cargo xtask checks --profile extended --only <id>` | 0 | `kani-core`, `fuzz-smoke`, `dependency-policy`, `coverage-report` each passed |

## Defects found

Both defects are fixed in `4a46b31`, with `38f4682` satisfying the core's arithmetic lint, and both are registered in [counterexamples.json](../formal/counterexamples.json).

- **`CX-RETENTION-SATURATED-TOTAL` (HIST-5).**
  - The defect: retention summed Segment sizes with saturation, then subtracted from the saturated total. With sizes `[u64::MAX, u64::MAX, 5]` and a 5-byte limit, it deleted one Segment and stopped while the rest still exceeded the limit.
  - The fix: the total is now an exact `u128`. An existing unit test had encoded the saturated behaviour, and now expects the exact answer.
- **`CX-RATE-NON-FINITE` (HIST-6).**
  - The defect: two `+inf` counter values gave a NaN rate, and an overflowing difference gave `+inf`, both on rows that are not resets. No answer can carry such a rate: a non-reset row needs a numeric rate, and JSON has no NaN.
  - The fix: such a pair is now a reset, and the [retained-history contract](../architecture/retained-history.md) says so. The differential test against the pre-extraction code accepts exactly this one divergence.

Both inputs are far outside real workloads, and proptest's generators never produced them. Kani checks the whole integer and float ranges.

## Not adopted

Loom, Shuttle, Miri, Creusot and Verus were not adopted. [ADR-0021](../decisions/ADR-0021-add-property-model-fuzz-and-simulation-checks.md) gives the reasons.
