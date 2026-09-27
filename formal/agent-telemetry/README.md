# Formal check: agent telemetry evidence

This optional development check covers the telemetry reducer's evidence merge. It does not model the Rust application, change the learning milestone, or run inside a Codex hook.

## Run

With Python and `uv` available, from the repository root:

```sh
uv run --no-project --python 3.13 --with z3-solver==4.13.3.0 python -B formal/agent-telemetry/check.py
```

To save a fresh evidence artifact, add `--output formal/agent-telemetry/results.json`. Alternatively, install [requirements.txt](requirements.txt) in an isolated Python environment and run `python -B formal/agent-telemetry/check.py`. Python 3.13 is used for the recorded run; the tool needs Linux because the imported telemetry module uses `fcntl`.

The implementation comparison also runs without Z3:

```sh
python3 -B formal/agent-telemetry/check_implementation.py
```

A successful full run exits zero only when all law queries have satisfiable assumptions and unsatisfiable negations, all deliberately faulty model variants have counterexamples, and the implementation comparison and its sensitivity controls pass. Solver `unknown` or a timeout is a failure, not evidence of validity. [Z3's validity guide](https://microsoft.github.io/z3guide/programming/Z3%20Python/Introduction/#satisfiability-and-validity) explains checking the negation of a claim.

## Model and correspondence

[check.py](check.py) models evidence for one operation with seven fields: observed start, observed finish, whether a name is known, the name label, whether a code is known, the code, and rejection. Unknown name/code payload values are unobservable. Name labels and codes range over arbitrary integers; names use equality only.

Compatible evidence combines by logical OR of observation flags and retention of the unique known name/code. Two different known names or concrete codes produce rejection. A rejection is absorbing: Python raises instead of returning a partial snapshot, so internal fields after rejection are not compared. The empty evidence record is the identity.

Under this mapping, joining compatible summaries corresponds to union of observed facts. Associativity, commutativity, and idempotence imply that any finite grouping, permutation, or repetition of compatible operation evidence has the same semantic result. Z3 checks those laws, preservation of known results, provenance of concrete codes, conflict detection, closure, monotonic flags, and isolation of a map entry keyed by the complete `(turn_id, operation_id)` pair. This is a symbolic result for the encoded algebra, not an automatic proof of Python execution.

[check_implementation.py](check_implementation.py) supplies the separate model-to-code evidence. Its reference was written from the contract without reading the reducer. It groups facts into sets and compares only `session_ended` and `turns`, including their operation records, against the actual [Python reducer](../../tools/telemetry/telemetry.py). It checks every trace of length 0–4 over eight observations:

- Bash start; Bash finish with unknown, zero, or one exit code; conflicting Read start;
- turn stop request; turn interruption; session end.

The 4,681 traces include permutations and repeated stable event IDs. Five targeted traces additionally check mixed sessions, reused operation IDs across turns, separate operations, conflicting event IDs, and retention of 61 operations beyond the recent-activity window. Rejection requires `ValueError`; an unrelated exception fails the check.

## Sensitivity and limits

Four Z3 negative controls deliberately overwrite known evidence with unknown, suppress conflict rejection, fabricate exit zero, or discard the turn part of an operation key. The solver must find a counterexample for each. Three implementation adapters deliberately return an empty projection, alias turns, or crash during rejection; the reference must detect each defect. These controls do not modify production code.

This proves neither that observations describe reality nor that every operation is observed. Stable identities, validated inputs, and authentic provenance remain assumptions. Missing hooks may remove evidence. There is no fairness assumption or temporal liveness claim.

Receipt-order history, timestamp parsing/formatting, schema validation, filenames, file locks, partial writes, process cancellation, hook trust, and model quality are outside the symbolic model. Existing contract and CLI tests cover some of these separately. The bounded implementation comparison is not a proof for all Python inputs or trace lengths. A future TLA+ model would need a specific temporal question and an explicit scheduler/failure model.

See the [registered investigation and results](../../docs/experiments/formal/agent-telemetry-merge.md), [raw report with source fingerprints](results.json), [architecture](../../docs/architecture/agent-telemetry.md), and [ADR-0003](../../docs/decisions/ADR-0003-observe-agent-activity-with-typed-events.md).
