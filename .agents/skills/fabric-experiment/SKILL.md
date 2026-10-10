---
name: fabric-experiment
description: Plan or carry out a scoped Fabric O11y performance ablation or formal correctness investigation, with explicit assumptions, reproducible commands, counterexamples, and limits. Use when a design claim needs evidence.
---

# Turn a design claim into evidence

Read the affected source, [current state](../../../docs/CURRENT.md), related decisions, and [experiment guidance](../../../docs/experiments/README.md). Follow the user's requested mode: a plan remains a plan until execution is authorized by the request. A performance aspiration is not permission to rebuild unrelated mechanisms, and a registered protocol is not changed after the fact: register a new revision before a new measurement. Qualification runs need explicit scope ([qualification](../../../docs/QUALIFICATION.md)).

For a measured comparison, state the hypothesis, fixed semantic contract, workload and seed, baseline, varied mechanism or parameter, metric definitions, measurement boundaries, and decision rule before running. Capture exact commands, revision or relevant working-tree diff, build settings, environment, raw results, and limitations. Include correctness and resource costs alongside throughput and latency; do not infer a universal winner from one workload.

For a correctness claim, state the invariant or temporal property and its assumptions. Choose the tool that answers that specific question: types for representable states, TLA+ for modeled transitions, Z3 for constraints, or an executable property for implementation behavior. Explain the model-to-code mapping, finite bounds, fairness assumptions if relevant, and what remains outside the model. Preserve counterexamples as deterministic fixtures and keep exact solver or checker outcomes. Every new checker needs a negative control it must reject. A bounded model result is not proof of arbitrary Rust executions.

Keep registered protocols, decision rules, regression fixtures and machine-consumed inputs under `docs/experiments/`. Publish research, results and observations in the [research wiki](https://github.com/kmosoti/FabricO11y/wiki), following the [documentation ownership and migration rules](../../../docs/documentation-policy.md#canonical-ownership). Link the report from the repository evidence index and affected architecture or ADR. Keep raw artifacts in controlled data-drive storage; never upload credentials or unreviewed telemetry. Pin the source revision and wiki commit for acceptance claims. Keep proposed procedures separate from executed results. Update [CURRENT.md](../../../docs/CURRENT.md) when evidence changes an active assumption or decision, and run the documentation check from the [contributor guide](../../../docs/CONTRIBUTING.md).
