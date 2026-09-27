# Formal check of agent telemetry aggregation

## Registered scope

This investigation concerns the optional Python agent-telemetry tooling, not the Rust ingestion/ownership milestone. The target is the operation-evidence merge in [the reducer](../../../tools/telemetry/telemetry.py). The implementation is inspected before modeling; an independent implementation-conformance reference is written from the contract without reading that code.

Claims to check before interpreting results:

1. Compatible observations merge associatively, commutatively, and idempotently, so order and repetition do not change operation facts.
2. A start or unknown finish cannot erase a concrete exit result.
3. A reported concrete code comes from an observed concrete code. In particular, unknown evidence cannot fabricate exit zero.
4. Different concrete codes or tool names for one operation lead to rejection.
5. Updates to one `(turn_id, operation_id)` do not change another operation's evidence.

The claims concern semantic facts. Receipt-order activity history is intentionally order-dependent. Timestamp spellings, schema parsing, whole-event identity validation, and session selection are outside the algebra; the implementation comparison separately exercises identity/session guards.

## Tool choice and assumptions

Z3 can check the merge laws for arbitrary symbolic integer code values without enumerating every integer. TLA+ would be useful for a future temporal claim about delivery, retries, locks, or fairness; this investigation claims none of those. No new runtime dependency is needed.

The model assumes validated inputs and stable identities. Tool names are abstract labels represented by integers; only equality is used. Null means no observed code. Contradictions map to an absorbing rejection outcome, corresponding to Python `ValueError`. States behind a rejection are unobservable and are compared as the same rejection outcome.

The input may be any observed subsequence of runtime activity. No fairness or eventual-delivery assumption is made. Consequently no liveness, completeness, durability, crash-recovery, filesystem-lock, hook-activation, or model-coding-quality guarantee is sought.

## Decision rule and sensitivity controls

For each proposed law, require satisfiable assumptions followed by `unsat` for its negation. Treat `sat`, `unknown`, timeout, or an unsatisfiable assumption domain as failure. Save concrete counterexamples for deliberately defective rules: overwrite known evidence with unknown, suppress contradictions, fabricate exit zero, and alias two turns by using only an operation ID.

Independently compare the actual reducer with a declarative grouping/sets reference for all traces of length zero through four over eight registered observations (4,681 traces). Add explicit cross-turn/operation/session and identity cases. Inject defective implementation adapters and require the reference check to reject them. This is a bounded correspondence check, not a proof of the Python program.

## Results

The recorded run used Python 3.13.5 and Z3 4.13.3 (Python package `z3-solver==4.13.3.0`). Command, executed from the repository root:

```sh
uv run --no-project --python 3.13 --with z3-solver==4.13.3.0 python -B formal/agent-telemetry/check.py --output formal/agent-telemetry/results.json
```

Exit status: **0**. The [raw report](../../../formal/agent-telemetry/results.json) preserves solver outcomes, complete input assignments for the merge counterexamples, and SHA-256 fingerprints of the model, independent reference, requirements, and checked implementation.

| Check | Outcome |
| --- | --- |
| 13 symbolic laws, including map isolation | Every assumption domain `sat`; every negated law `unsat` |
| Four deliberately faulty model variants | Every negated property `sat`, with a saved counterexample |
| All 4,681 traces of length 0–4 | Actual reducer matched the independent reference: 2,549 accepted, 2,132 correctly rejected |
| Five targeted implementation traces | Three accepted, two correctly rejected |
| Empty projection, aliased turns, and crash-on-rejection implementation adapters | All three injected defects detected |

The solver found, for example, that suppressing conflict rejection accepts two known codes `2` and `3` for the same named operation. Dropping the turn part of a key let an update for turn `2`, operation `4` change turn `3`, operation `4` from `6` to `5`. These are counterexamples to deliberately defective designs, not findings against the current reducer.

The independent reference initially allowed arbitrary exceptions to count as rejections. Review tightened this to require `ValueError`, then injected `RuntimeError` to verify that accidental crashes cannot masquerade as correct behavior. The production reducer needed no changes for this investigation.

## Interpretation and limits

The algebraic results range over arbitrary integer codes and abstract name labels. Associativity, commutativity, and idempotence extend by induction to finite folds of compatible evidence in the model. Rejection is compared as one absorbing outcome because no partial snapshot is returned. The map-isolation theorem assumes the complete pair key; the bounded reference checks how the actual reducer uses its keys.

The Python correspondence check remains bounded to the registered alphabet/traces and five targeted cases. It is not a mechanized refinement proof of every Python execution. Lifecycle flags appear in the finite reference check; the symbolic merge laws concern operation evidence. Receipt-order history is excluded because its order is meaningful.

This investigation does not verify parsing, timestamp handling, filesystem or operating-system behavior, cancellation, live Codex hook activation, eventual delivery, or effects on reasoning quality. Those claims need separate evidence. No assumptions about future services or Rust delivery semantics are introduced.

## Decision impact

The results support keeping the current merge rule: preserve known evidence, reject contradictions, and isolate operations by turn and operation ID. The solver runs only as an offline development check; this investigation makes no recommendation about synchronization. The [model guide](../../../formal/agent-telemetry/README.md) records the mapping and reproduction steps; [ADR-0003](../../decisions/ADR-0003-observe-agent-activity-with-typed-events.md) remains the applicable decision.
