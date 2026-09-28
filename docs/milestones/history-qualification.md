# Milestone: history qualification

Status: complete pending CI and merge. Base: the semantic-kernels milestone at `5a3e289`.

This milestone runs the registered history protocol. Only one host is available, with four CPUs, so it first registers [revision 2](../experiments/benchmarks/history-protocol-r2.md), before any run. Its results are therefore **measurements on a four-CPU host, not a target-profile qualification**. It changes no wire format, persisted format, oracle or product-contract gate.

## Acceptance criteria

| ID | Criterion | Deciding evidence |
| --- | --- | --- |
| HQ-1 | Any change to the registered protocol is registered before a run under it, in its own commit, and keeps the defaults of revision 1 | [Revision 2](../experiments/benchmarks/history-protocol-r2.md) (`63bbeac`). `history_tier.py` defaults reproduce revision 1's placement exactly |
| HQ-2 | The binaries and harness are frozen with hashes before the run, and are unchanged after it | [hashes.txt](../experiments/benchmarks/data/history-qualification/hashes.txt); `sha256sum -c` after the run |
| HQ-3 | Every registered trial runs to a result under the runner, with no budget stop | four runner results: exit 0, `passed=true`, no stop reason |
| HQ-4 | Query gate: every graded answer passes the frozen query oracle, and each kind's p99 is at most 2 s | [history run 01](../experiments/benchmarks/history-run-01.md) |
| HQ-5 | Freshness gate: p99 is at most 5 s in each Segment-mode trial | history run 01 |
| HQ-6 | Journal-only and Segment reads are compared on latency per query kind and on live bytes | history run 01 |
| HQ-7 | Failed or partial attempts are kept, not hidden | the failed start and its [erratum](../experiments/benchmarks/history-protocol-r2.md#erratum-before-any-measurement) |
| HQ-8 | The records are updated, and none of them calls a result that is not target-profile "Qualified" | the capability ledger; the verification matrix; the retained-history view; [ADR-0020](../decisions/ADR-0020-store-sealed-history-as-parquet-segments.md), which records the Segment format (plan item 4.6); the experiments index; the roadmap; the current state. The docs check exits 0 |

Out of scope, stated so it is not implied:

- **Revision 1 on the target host.** It needs a 12-CPU Debian 13 host, which is not available here.
- **A token index.** Plan item 4.5 adds one only if the query gate fails, and it did not fail.

## Definition of done

HQ-1 to HQ-8 are met, with the commands and exits recorded below. The PR's CI is green (Rust, Documentation, Extended verification), and the PR is merged into `main`.

## Results

Host: 4 logical CPUs, 15 GiB RAM, Ubuntu 24.04 (Firecracker VM), ext4, rustc 1.94.1, Python 3.11.15, Bun 1.4.0.

| Criterion | Command | Exit | Result |
| --- | --- | --- | --- |
| HQ-1 | commit `63bbeac`, before any trial | — | revision 2 registered: server on CPUs 0-1, simulator on 2-3, everything else unchanged |
| HQ-2 | `cargo build --release --locked --workspace --bins --examples` at `63bbeac`, then copy and `sha256sum` | 0 | 23 hashes; `sha256sum -c hashes.txt` after the run: 0 mismatches |
| HQ-3, first attempt | the four runner invocations with relative harness paths | 2 (each) | measured nothing: the runner starts the child in the output directory. Kept under `failed-start`; erratum `e3ef669` |
| HQ-3 | the four runner invocations in [history run 01](../experiments/benchmarks/history-run-01.md#method) | 0 (each) | `passed=true`, no stop reason, cleanup confirmed; 379–477 s per trial |
| HQ-4 | the same | 0 | every graded answer passed the oracle (4,138 rows per trial). Largest p99: 481 ms with Segments, 1,515 ms journal-only |
| HQ-5 | the same | 0 | freshness p99 of 1.70, 1.78 and 1.66 s |
| HQ-6 | the same | 0 | Segments about 3 to 4 times faster at p99; 242 MiB against 304 MiB live |
| HQ-8 | `bun tools/docs/check.mjs` | 0 | Documentation checks passed |
| HQ-8 | `cargo xtask checks --profile fast` | 0 | 16 of 16 |

Evidence is in [data/history-qualification](../experiments/benchmarks/data/history-qualification/hashes.txt).

State after this milestone:

- **History query latency, freshness, and the journal-versus-Segment comparison:** Measured and passing under revision 2 on `63bbeac`.
- **Target profile:** not qualified.
