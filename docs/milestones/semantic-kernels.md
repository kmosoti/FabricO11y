# Milestone: semantic kernel extraction

Status: complete pending CI and merge. Base: `main` at `04dbfe4` (verification foundation merged). This milestone moves domain decisions into `fabric-core` without changing behavior; it changes no wire or persisted format and is not qualification.

## Acceptance criteria

| ID | Criterion | Deciding evidence |
| --- | --- | --- |
| SK-1 | Control decisions (status transitions, revision increment, authorization, name and desired-configuration limits) are a `no_std` kernel used by the server | `fabric_core::control`; the server's `Control` calls it; the differential test agrees with the code it replaced |
| SK-2 | Query decisions (window, limit, snapshot, the Gone rule, page continuation, completeness, counter step) are a kernel used by the server | `fabric_core::query`; `History::run` and `rates` call it; the counter step matches the base bit for bit |
| SK-3 | Retention eligibility is a kernel applied by a use case over a port | `fabric_core::retention`, `fabric_app::retention::apply_retention`, `SegmentStore`; the sealer implements the port |
| SK-4 | Collection decisions (counter start, log-cursor continuation, gap-text bound) are a kernel used by the Spindle | `fabric_core::collection`; the runtime and log reader call it |
| SK-5 | The Spindle's Linux effects are behind an adapter crate | `fabric-adapter-linux` in the adapter layer; `cargo xtask check-layers` passes |
| SK-6 | Each kernel has a semantic mutant its named checker catches, and moved mutants still target live code | `cargo xtask mutants`: every mutant caught |
| SK-7 | Behavior is preserved | the fast profile passes; the extended profile passes (delivery faults with TLC trace validation, cargo-mutants audit including the new kernels) |
| SK-8 | Documents agree | system view, layer diagram, control and retained-history views, Spindle view, verification matrix, roadmap and current state; docs check exit 0 |

Out of scope, stated so it is not implied: splitting `fabric-server` into an adapter crate and a thin root, and moving the Spindle runtime and Spool out of the root package. Both composition roots keep their adapters; their decisions are now kernels.

## Definition of done

SK-1 to SK-8 met with commands and exits recorded below, the PR's CI green (Rust, Documentation, Extended verification), and the PR merged into `main`.

## Results

Local runs: rustc 1.94.1, Python 3.11.15, Bun 1.4.0, TLC v1.7.1, cargo-mutants 27.1.0, 4-CPU container. Evidence files are under [data/semantic-kernels](../experiments/benchmarks/data/semantic-kernels/cargo-mutants-audit.txt).

| Criterion | Command | Exit | Result |
| --- | --- | --- | --- |
| SK-1 to SK-4 | `cargo test -p fabric-core --test legacy_kernels` | 0 | every status pair and four revisions; 2,500 counter pairs bit for bit; 69,904 retention histories; all small counter-start cases including NaN and type changes; 960 cursor cases; multibyte gap texts at caps 0 to 20: no disagreement with the base code |
| SK-1 to SK-4 | `cargo test --workspace --locked --all-features` | 0 | 139 tests at the adapter split; kernels used by the server and the Spindle; the query-oracle-graded history tests and the Spindle tests pass |
| SK-3 | `cargo test -p fabric-app --test retention` | 0 | 3 use-case tests; the server retention test runs through `SegmentStore` |
| SK-5 | `cargo xtask check-layers` | 0 | `fabric-adapter-linux` in the adapter layer |
| SK-6 | `cargo xtask mutants`, first run | 1 | 16 of 17 caught; **M-COL-RESET survived**: the Spindle counter test asserted `start_reset >= start_a`, which the defect satisfies. The assertion is now strict (`872ba5e` before rebase, recorded as `CX-COUNTER-RESET-ASSERTION`) |
| SK-6 | `cargo xtask mutants` after the fix | 0 | 17 of 17 caught (extended run on the rebased tree) |
| SK-7 | `cargo xtask checks --profile fast` | 0 | 16 of 16 |
| SK-7 | `cargo xtask checks --profile extended`, first run on the rebased tree | 1 | semantic mutants, delivery faults with TLC trace validation, both TLA+ models and the trace model passed; **cargo-mutants failed**: three `check_desired` boundary mutants survived (no test at exactly 240 bytes or 16 paths), and the audit merged two distinct survivors because it dropped their positions |
| SK-7 | `cargo xtask cargo-mutants` after the boundary tests and the audit fix | 0 | 165 mutants: 154 caught, 9 unviable, 1 detected by timeout (`bounded_text` loop negation), 1 missed and listed as equivalent (`delivery.rs:95:24`) |
| SK-8 | `bun tools/docs/check.mjs` | 0 | Documentation checks passed |

Counterexamples kept: the control revision `revision + 1` overflow (now `next_revision`), the weak counter-start assertion, and the untested `check_desired` boundaries.

Not done, and not claimed: the `fabric-server` crate is still one composition root holding its adapters, and the Spindle runtime and Spool stay in the root package.
