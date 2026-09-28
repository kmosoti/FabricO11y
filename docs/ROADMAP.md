# Roadmap

Planning is by milestone: a stable engineering objective with its own branch `milestone/<capability>`. A pull request is only how a milestone's change is reviewed, so documents name milestones, not pull-request numbers, model names, session IDs or release maturity. Future milestone branches are created when their work starts. Release maturity is a tag on a revision ([ADR-0019](decisions/ADR-0019-keep-release-maturity-in-tags.md)).

| Order | Milestone | Branch | Objective | Deciding evidence |
| --- | --- | --- | --- | --- |
| 1 | Architecture foundation | `milestone/architecture-foundation` | Hexagonal layers with a checked dependency rule, a pure `no_std` core, the first kernel (delivery), capability-oriented documents, evidence-based workflow | [milestone record](milestones/architecture-foundation.md) |
| 2 | Verification foundation | `milestone/verification-foundation` | Delivery fault-run traces validated by TLC against the model, direct journal/Segment metamorphic tests, a counterexample registry, a `cargo-mutants` audit, scheduled extended verification with receipts | [milestone record](milestones/verification-foundation.md) |
| 3 | Semantic kernel extraction | `milestone/semantic-kernels` | Move control, query, retention and collection decisions into `fabric-core`; retention as a use case over a port; the Spindle's Linux reads in an adapter crate | [milestone record](milestones/semantic-kernels.md) |
| 4 | History qualification | `milestone/history-qualification` | Run the registered [history protocol](experiments/benchmarks/alpha-phase4-history-protocol.md): 1,000,000-record query latency, freshness at 1,000 identities, journal-only versus Segment reads | [milestone record](milestones/history-qualification.md): measured under [revision 2](experiments/benchmarks/history-protocol-r2.md) on a four-CPU host; revision 1 on the target host remains |
| 5 | Delivery and recovery qualification | `milestone/delivery-recovery` | Run the registered [outage](experiments/benchmarks/alpha-phase5-outage-protocol.md) and [stress](experiments/benchmarks/alpha-phase5-stress-protocol.md) protocols; register and run a soak | run records |
| 6 | Linux installation qualification | `milestone/linux-installation` | Running-installation acceptance on a disposable systemd host | the acceptance list in the [product contract](PRODUCT-CONTRACT.md#linux-installation-contract) |
| 7 | Release readiness | `milestone/release-readiness` | Operator reference, recovery guide, checksums; every gate in the [capability ledger](QUALIFICATION.md#capability-ledger) has a recorded command, exit and evidence; then a tag | the ledger |

The historical source branch `alpha/controlled-linux-collection` stays available until its work is integrated; it is not deleted automatically.

## Work carried from the completion plan

The release-stage completion plan (`docs/ALPHA-PLAN.md` at base `9b3a2b4`, readable with `git show 9b3a2b4:docs/ALPHA-PLAN.md`) is retired. Its settled decisions and open steps continue here:

| Plan item | Where it lives now |
| --- | --- |
| D1 interrupted append versus known failure | Implemented; [ADR-0011](decisions/ADR-0011-separate-interrupted-append-from-known-failure.md) |
| D2 clearing coverage-unknown | Implemented; described in the [Spindle view](architecture/spindle.md) |
| D3 workspace and dependencies | [ADR-0012](decisions/ADR-0012-add-a-server-crate-in-a-workspace.md), refined by [ADR-0015](decisions/ADR-0015-adopt-a-hexagonal-architecture.md) |
| D4 delivery and deduplication rule | [ADR-0013](decisions/ADR-0013-deliver-batches-in-order-with-bounded-dedup.md); now the `fabric-core` delivery kernel |
| D5 Spool reclaim after ACK | Implemented; [storage view](architecture/storage.md) |
| 4.5 indexes: row-group statistics only; a token index only if the 1,000,000-record gate fails, with exact-scan fallback | History qualification |
| 4.6 journal-only versus Segment-backed reads, docs and a Segment-format ADR | History qualification |
| 5.3 fleet tiers with one real server, outage and recovery, 30-minute buffering, 5× bursts, overload, auth rejection, concurrent management and query, soak within the 2 h limit | Delivery and recovery qualification |
| 5.4 CLI and API reference, recovery guide, examples, checksums, tag only after every gate has evidence | Release readiness |
| Cross-family language-model review at each gate | Removed as a gate ([ADR-0018](decisions/ADR-0018-accept-work-on-executable-evidence.md)); past findings are kept as regressions |

Carried rules: every measurement runs under the [runner](../tools/qualification/runner.py); evidence stays compact (manifests, hashes, exits, histograms, minimized counterexamples) under `docs/experiments/benchmarks/data/`, bulk data under ignored `target/alpha-*`; the non-goals in the [product contract](PRODUCT-CONTRACT.md#non-goals-of-the-first-profile) hold.

## Known risks

- WSL2 and ext4 sync behavior is the only tested environment; power loss stays untested. The I/O controller may be ineffective on WSL; report it, never count it as enforcement.
- One host runs the 1,000-identity tier, the server and the harness; CPU contention distorts p99. Record host state and keep tiers sequential.
- `parquet` 60 raises server build time and memory.
- Scope creep: any new configuration key without a gate consumer is a defect.
- `fabric-server` is still one composition-root crate that also holds its adapters (journal, Segments, control persistence, query reads), and the Spindle runtime and Spool live in the root package; the layer gate cannot see inside them. The decisions they make are now kernels in `fabric-core`; splitting the server crate itself remains open.
