# Experiments

The active [end-to-end completion contract](ablation/end-to-end-prototype.md) tracks
remaining gates. [E3R](ablation/resume-e3-protocol.md) registers snapshot-bound retries;
[S0](benchmarks/append-attribution-s0.md) registers append phase attribution.
The [E2R result](ablation/seal-e2-run-01.md) records the checked sector/error model,
counterexamples and repair; its [cost result](benchmarks/group-seal-cost-run-01.md) reports mixed CPU/throughput
gates under the separate [protocol](benchmarks/group-seal-cost-protocol.md). [S2](ablation/durable-snapshot-s2-protocol.md) registers the next durable
snapshot/checkpoint boundary.

The [observability storage agenda](ablation/observability-storage-research.md) contains the corrected survey and staged storage/query experiments. [S1](ablation/storage-query-s1-run-01.md) implements an exact scan oracle and optional block summaries over a replayed in-memory snapshot; its [protocol](ablation/storage-query-s1-protocol.md) fixes workloads and gates. It does not migrate storage or measure disk pruning. The [E1R coverage result](ablation/coverage-e1-run-01.md) records a separate metadata-authentication correctness cell, its [protocol](ablation/coverage-e1-protocol.md), independent oracle and failing mutations. It distinguishes query completeness from raw retention.

The [original Stage 3 batch-size probe](benchmarks/batch-size-stage3.md) compares four batch sizes using the archived hand-written generator and selects no winner. The [generator library ablation](benchmarks/generator-library-stage3.md) compares that archived generator with the current `fake` adapter. The [Stage 4 delivery model](formal/delivery-ownership.md) checks a target ACK rule with TLC; [Stage 5 implementation checks](formal/delivery-rust-stage5.md) exercise the local log and recovery path. The [Stage 6 local-log baseline](benchmarks/local-log-stage6.md) preregisters and reports one measured workload with preserved raw samples; it selects no optimization. The [Homa/SIRD receiver-driven transport study](ablation/receiver-driven-transport.md) remains the broader research plan. Its [H1 receiver-credit comparison](ablation/receiver-credit-h1-run-01.md), [M2 unscheduled-prefix comparison](ablation/unscheduled-prefix-m2-run-01.md), and [finite credit/ownership check](formal/transport-credit-ownership.md) now have results, but no network protocol or real-host transport measurement exists. Separately, the [agent telemetry formal check](formal/agent-telemetry-merge.md) verifies a development tool's evidence algebra with Z3 and a bounded implementation comparison. Numbers in the [blueprint](../architecture.md) are illustrative examples, not Fabric O11y measurements.

For a performance experiment, record the hypothesis, workload and seed, compared contract, baseline, changed variable, exact commands, toolchain and machine, metrics, results, interpretation, limitations, and decision impact. Define the metrics before running comparisons. Preserve raw results when a conclusion depends on them.

For a formal claim, state the property, assumptions, model-to-code correspondence, explored bounds, exact tool version and command, and counterexamples or result. A bounded check of a model does not establish unbounded correctness of the Rust implementation.

The repeatable input, bounded buffer, and local commit/replay path are implemented. A broader comparison still needs a representative workload and chosen resource and latency metrics. The [learning path](../LEARNING_PATH.md) moves from the delivery model and log into measurement. Use the [experiment skill](../../.agents/skills/fabric-experiment/SKILL.md) for a scoped investigation.

Create an experiment document when there is an actual hypothesis to evaluate. Link resulting evidence from the relevant architecture page and ADR. Documentation-tool checks belong in the [contributor workflow](../CONTRIBUTING.md); they are not application performance experiments.
