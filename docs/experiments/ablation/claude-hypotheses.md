# Claude hypothesis study: provenance and disposition

On 2026-09-27, Claude Opus used the `/mad-scientist` research workflow in a separate study to propose ways to preserve exact observability query completeness while reducing ingestion or query cost. The [portable archive](data/claude-ideas-2026-09-27/README.md) preserves the initial ideas, experiment registrations, critiques, conceptual probes, run metadata and hashes. These were hypotheses; the initial study did not run E1R, E2R or E3R. Its `not_run` ledger is a historical snapshot, not the status of later Fabric experiments.

The initial six proposals were I1 authenticated coverage receipts, I2 a one-sync group seal, I3 residual answers for unavailable blocks, I4 IBLT reconciliation of ACK and owner ledgers, I5 a commit marker that also holds a query summary, and I6 lossless log-template decomposition as an exact absence certificate. A GPT-family independent critique ran six small [counterexample probes](data/claude-ideas-2026-09-27/critique_probes.py) and requested changes. These probes test logical claims, not crash durability or performance.

| Initial proposal | Counterexample and disposition | Repaired study proposal and later Fabric evidence |
| --- | --- | --- |
| I1 coverage receipts | A fresh authenticated seal can bind a false-negative summary. Hashes alone do not prove summary semantics. The original claim was killed. | **I7 → E1R:** validate summaries against rows in a trusted builder, then verify receipts against an independently retained root and block count. The [E1R result](coverage-e1-run-01.md) passed its finite correctness corpus under those trust assumptions. A deliberately bad sealed summary still passes the row-free receipt verifier and fails the independent row oracle. |
| I2 one-sync group seal | Truncating a full, mismatching seal could discard an ACKed group. The original claim was killed. | **I8 → E2R:** a full mismatching seal fails closed and preserves bytes; an external caller retains knowledge of failed I/O. The [E2R result](seal-e2-run-01.md) checked a byte/sector model and reviewed a repaired candidate. It does not establish physical power-loss durability. |
| I3 residual answers | Merging a partial answer from snapshot S with a residual from compacted snapshot T can produce an answer from neither. The original claim was killed. | **I9 → E3R:** bind predicate, snapshot head, block digests and physical row coordinates; reject conflicting values at the same coordinate. The [E3R result](resume-e3-run-01.md) reports zero violations in its fixed finite corpus and independent probes for the selected research candidate. Its integrated Serde form did not receive a separate full-corpus rerun. |
| I4 IBLT ACK audit | Two stale ledgers can agree while retained raw data is gone. Parked until independently verified owner inventory and a durable sender ACK history exist. | No detector or application protocol is established by this study. |
| I5 summary inside commit marker | A single writer blocked in sync cannot compute a post-write summary during that same sync; co-location also cannot make a wrong summary sound. Parked pending an explicit computation schedule and equal-batching cost comparison. | No performance result follows from the original idea. |
| I6 template absence certificate | `time{v}` with `v=out` reconstructs `timeout`, but neither component contains that query token. The original exact-pruning claim was killed. | Token-aware reconstruction or indexing is known engineering, not a demonstrated new method here. |

A second independent GPT-family review found further boundary cases in I7–I9. Its [revision probes](data/claude-ideas-2026-09-27/revision_probes.py) show that a row-free receipt verifier cannot detect a trusted builder's semantic bug; a seal-only persisted sector must error and preserve bytes; successful and failed syncs can leave identical file bytes; and repeated rows at one residual coordinate can conflict. The [amended registrations](data/claude-ideas-2026-09-27/KILL_TESTS.md) incorporated these cases before any study experiment ran. The [final conceptual review](data/claude-ideas-2026-09-27/FINAL_REVIEW.md) approved the *testability* of those amended proposals, not an implementation. The original generation and later Fabric implementation reviews used Claude models; the study's adversarial critiques were GPT-family. The later [E1R](coverage-e1-run-01.md) and [E2R](seal-e2-run-01.md) records document their own cross-family implementation reviews, including Claude Sonnet where identified.

These mechanisms have close prior art in authenticated query verification, journal commit records, and incomplete-answer semantics. The study's embedding scores were a search aid, not evidence of novelty. The narrow contribution tested here is the stated Fabric contract under explicit assumptions. E1R depends on a correct builder, independently trusted root and honest exact evaluator; E2R depends on the modeled sector and sync assumptions plus external failed-I/O knowledge; E3R depends on immutable snapshot identity and a trusted exact evaluator. No study probe establishes an application deployment, a malicious-executor proof, physical durability, or a cost win.

## Claude session handoff

| Session | Purpose and result | Process state |
| --- | --- | --- |
| `e5c74bcc-0707-41f8-aff8-a1a1d57552ab` | Opus `/mad-scientist` generation; repaired hypotheses registered after independent critiques | Exited 0; [original receipt](data/claude-ideas-2026-09-27/CLAUDE_RUN.json) |
| `d756107f-c6d6-4d10-9f13-d5e484a8db17` | Sonnet implementation reviews for seals, residuals, disk, layout and collection/CLI; repaired candidates approved after probes | One-shot invocations exited; see the linked component result records |
| `555a7e89-f6f4-423e-8745-77e98acec3ff` | Sonnet CLI-cap and cost-harness repair review; final repaired harness approved | Exited 0; [final cost review](../benchmarks/data/research-costs-run-01/review/claude-final/stdout.json) |

Some early Git reads and shell-command variants were denied by the scoped tool
permissions. Their responses retain those denials; permitted probes or parent-run
checks supplied the required evidence. The final cost-harness review recorded zero
permission denials. Claude wrote its external study and scratch review probes;
repository implementation repairs were integrated by the parent/worker workflow.
No research Remote Control server was started, and no invoked one-shot review remains
running. A review approval is separate from the actual command exits and later formal
cost-result audit.
