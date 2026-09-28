# Milestone: verification foundation

Status: complete; merged into `main` by PR #20 (`04dbfe4`) with Rust, Documentation and Extended verification CI green on the head `81b738b`. Base: `main` at `6521e06` (architecture foundation merged). This milestone adds verification machinery; it changes no product behavior and is not qualification.

## Acceptance criteria

Each criterion names the evidence that decides it. A criterion is met only when that evidence exists on this branch.

| ID | Criterion | Deciding evidence |
| --- | --- | --- |
| VF-1 | Delivery fault-run transcripts are mapped onto the `DeliveryOwnership` TLA+ actions and validated by TLC; implementation behavior with no legal modeled transition is rejected | `formal/delivery/trace_check.py`; a real transcript is accepted; at least three injected violations are rejected (`formal/delivery/test_trace_check.py`) |
| VF-2 | The journal/Segment equivalence and seal-invariance relations (HIST-1, HIST-2) are checked directly | a server test comparing the same queries before and after sealing; a semantic mutant on the Segment read path that it must catch |
| VF-3 | Counterexamples are a registry, not prose | `docs/formal/counterexamples.json`; `cargo xtask check-counterexamples` in the fast profile, with negative controls |
| VF-4 | `cargo-mutants` runs on core and app with an explicit equivalence list | `cargo xtask cargo-mutants` passes; an empty list fails it |
| VF-5 | Extended verification runs in CI on a schedule, on demand, and on PRs touching verified code | `.github/workflows/verification.yml` runs `cargo xtask checks --profile extended` successfully on this PR |
| VF-6 | Fast CI still passes and keeps receipts | the Rust workflow succeeds on this PR |
| VF-7 | Documents agree | verification strategy, matrix, roadmap and current state updated; `bun tools/docs/check.mjs` exit 0 |

## Definition of done

All of VF-1 to VF-7 met, the PR's CI green (Rust, Documentation, Extended verification), every result recorded below with its command and exit status, and the PR merged into `main`.

## Results

Local runs used rustc 1.94.1, Python 3.11.15, Bun 1.4.0, TLC v1.7.1 (SHA-256 verified), cargo-mutants 27.1.0 and OpenJDK 21, on a 4-CPU container.

| Criterion | Command | Exit | Result |
| --- | --- | --- | --- |
| VF-1 | `TLA_JAR=… python3 -B formal/delivery/test_trace_check.py` | 0 | 5 tests: the real server-kill transcript and a legal forget are accepted; early forget, ACKed identity missing after recovery, and resend after forget are rejected |
| VF-1 | extended `delivery-faults`: 8 fault runs, each transcript then checked by `trace_check.py` | 0 | all 8 pass the delivery oracle and are accepted by TLC; the `drop-recovered` control fails as intended |
| VF-2 | `cargo test -p fabric-server --test history journal_and_segment_representations_answer_identically` | 0 | journal-only and Segment answers are equal for 4 queries |
| VF-2 | `cargo xtask mutants --only M-HIST-SEGMENT` | 0 | caught by that test |
| VF-3 | `cargo xtask check-counterexamples`; `cargo test -p xtask --test evidence` | 0; 0 | 11 entries valid; a missing reproducer and an unknown commit each fail |
| VF-4 | `cargo xtask cargo-mutants` | 0 | 34 mutants: 27 caught, 6 unviable, 1 listed equivalent |
| VF-4 | the same with the equivalence list emptied | 1 | `MUTANT_SURVIVED` for the equivalent mutant, as intended |
| VF-5 | `cargo xtask checks --profile extended` (locally) | 0 | semantic mutants (13 of 13 caught), delivery faults with trace validation, both TLA+ models, trace model, cargo-mutants |
| VF-6 | `cargo xtask checks --profile fast` (locally) | 0 | 16 of 16 passed, including the new counterexample check |
| VF-7 | `bun tools/docs/check.mjs` | 0 | Documentation checks passed |

The local extended receipts are kept under [data/verification-foundation](../experiments/benchmarks/data/verification-foundation/extended-checks.txt). They name `6521e06` with a dirty tree because the run started before the milestone's commits; its content is the tree of those commits. The receipts from the PR's CI runs record the committed head. CI results on the PR are recorded in the PR itself.

Not done here: property-based testing is still not adopted, and trace validation covers the delivery model only.
