---
name: fabric-experiment
description: Plan or carry out a scoped Fabric O11y performance ablation or formal correctness investigation, with explicit assumptions, reproducible commands, counterexamples, and limits. Use when a design claim needs evidence.
---

# Turn a design claim into evidence

Read the affected source, [current state](../../../docs/CURRENT.md), related decisions, and [experiment guidance](../../../docs/experiments/README.md). Follow the user's requested mode: a plan remains a plan until execution is authorized by the request. A performance aspiration is not permission to rebuild unrelated mechanisms.

For a measured comparison, state the hypothesis, fixed semantic contract, workload and seed, baseline, varied mechanism or parameter, metric definitions, measurement boundaries, and decision rule before running. Capture exact commands, revision or relevant working-tree diff, build settings, environment, raw results, and limitations. Include correctness and resource costs alongside throughput and latency; do not infer a universal winner from one workload.

For a correctness claim, state the invariant or temporal property and its assumptions. Choose the tool that answers that specific question: types for representable states, TLA+ for modeled transitions, Z3 for constraints, or an executable property for implementation behavior. Explain the model-to-code mapping, finite bounds, fairness assumptions if relevant, and what remains outside the model. Preserve counterexamples and exact solver or checker outcomes. A bounded model result is not proof of arbitrary Rust executions.

Store the scoped record under `docs/experiments/` when it becomes concrete, and link code, models, raw artifacts, architecture, and any affected ADR using relative links. Keep proposed procedures separate from executed results. Update [CURRENT.md](../../../docs/CURRENT.md) when evidence changes an active assumption or decision, and run the documentation check from the [contributor guide](../../../docs/CONTRIBUTING.md).
