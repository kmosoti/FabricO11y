# Experiments

The [Linux alpha phase-0 FOL2 baseline](benchmarks/alpha-phase0-baseline.md) records
fresh local commands and limits. The [alpha contract](../ALPHA.md) keeps later
fleet thresholds pending.
The [alpha draft journal length repair](formal/alpha-journal-length-repair.md) records a
frozen corruption counterexample and its narrow rerun; it is not phase-1 qualification.
The [native-node protocol](benchmarks/alpha-phase1-native-protocol.md) registers the
phase-1 workload and metrics before [three measured local trials](benchmarks/alpha-phase1-native-run-01.md).
The [post-repair native run 02](benchmarks/alpha-phase1-native-run-02.md) supersedes run 01 as phase-1 native evidence.
The [log-reader repair](formal/alpha-log-reader-repair.md) records independent
oversized-line and FIFO counterexamples and their executable controls.
The [phase-1 review checkpoint](formal/alpha-phase1-review-checkpoint.md) records
the later focused repairs, executable probes, the approved re-review and the open
counterexample. Frozen reviewer probes, verdicts and parent notes are kept under
[data/alpha-review](benchmarks/data/alpha-review/README.md).

The [local prototype completion contract](ablation/end-to-end-prototype.md) is closed
with linked evidence. [Claude's hypothesis study](ablation/claude-hypotheses.md) traces
the original proposals and counterexamples to the later registered experiments.
[E3R](ablation/resume-e3-run-01.md) checks snapshot-bound retries;
[S0](benchmarks/append-attribution-s0.md) measures append attribution with a perturbation
flag. The [E2R model](ablation/seal-e2-run-01.md) and
[cost result](benchmarks/group-seal-cost-run-01.md) preserve its repaired contract and
mixed CPU/throughput gates.

[S2](ablation/durable-snapshot-s2-run-01.md) checks durable snapshot/checkpoint behavior.
The [local lifecycle](ablation/local-prototype-run-01.md) checks offline collection,
bounded ingestion and separate-process query/resume. [S3/S4 correctness](ablation/columnar-selective-s3-s4-run-01.md)
checks hybrid layout and postings. [Measured receipt/layout/sidecar costs](benchmarks/research-costs-run-01.md)
include favorable and unfavorable results under the registered
[receipt](benchmarks/receipt-resume-cost-protocol.md),
[layout](benchmarks/columnar-selective-s3-s4-protocol.md) and
[sidecar](benchmarks/collection-sidecar-s5-protocol.md) protocols. JSON remains the
prototype lifecycle baseline; these synthetic results select no production format.

The [observability storage agenda](ablation/observability-storage-research.md) contains the corrected survey and staged storage/query experiments. [S1](ablation/storage-query-s1-run-01.md) implements an exact scan oracle and optional block summaries over a replayed in-memory snapshot; its [protocol](ablation/storage-query-s1-protocol.md) fixes workloads and gates. It does not migrate storage or measure disk pruning. The [E1R coverage result](ablation/coverage-e1-run-01.md) records a separate metadata-authentication correctness cell, its [protocol](ablation/coverage-e1-protocol.md), independent oracle and failing mutations. It distinguishes query completeness from raw retention.

The [original Stage 3 batch-size probe](benchmarks/batch-size-stage3.md) compares four batch sizes using the archived hand-written generator and selects no winner. The [generator library ablation](benchmarks/generator-library-stage3.md) compares that archived generator with the current `fake` adapter. The [Stage 4 delivery model](formal/delivery-ownership.md) checks a target ACK rule with TLC; [Stage 5 implementation checks](formal/delivery-rust-stage5.md) exercise the local log and recovery path. The [Stage 6 local-log baseline](benchmarks/local-log-stage6.md) preregisters and reports one measured workload with preserved raw samples; it selects no optimization. The [Homa/SIRD receiver-driven transport study](ablation/receiver-driven-transport.md) remains the broader research plan. Its [H1 receiver-credit comparison](ablation/receiver-credit-h1-run-01.md), [M2 unscheduled-prefix comparison](ablation/unscheduled-prefix-m2-run-01.md), and [finite credit/ownership check](formal/transport-credit-ownership.md) now have results, but no network protocol or real-host transport measurement exists. Separately, the [agent telemetry formal check](formal/agent-telemetry-merge.md) verifies a development tool's evidence algebra with Z3 and a bounded implementation comparison. Numbers in the [blueprint](../architecture.md) are illustrative examples, not Fabric O11y measurements.

For a performance experiment, record the hypothesis, workload and seed, compared contract, baseline, changed variable, exact commands, toolchain and machine, metrics, results, interpretation, limitations, and decision impact. Define the metrics before running comparisons. Preserve raw results when a conclusion depends on them.

For a formal claim, state the property, assumptions, model-to-code correspondence, explored bounds, exact tool version and command, and counterexamples or result. A bounded check of a model does not establish unbounded correctness of the Rust implementation.

The repeatable input, bounded buffer, and local commit/replay path are implemented. A broader comparison still needs a representative workload and chosen resource and latency metrics. The [learning path](../LEARNING_PATH.md) moves from the delivery model and log into measurement. Use the [experiment skill](../../.agents/skills/fabric-experiment/SKILL.md) for a scoped investigation.

Create an experiment document when there is an actual hypothesis to evaluate. Link resulting evidence from the relevant architecture page and ADR. Documentation-tool checks belong in the [contributor workflow](../CONTRIBUTING.md); they are not application performance experiments.
