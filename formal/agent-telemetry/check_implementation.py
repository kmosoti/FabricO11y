"""Finite conformance checks for the telemetry reducer's specified projection.

This reference is intentionally declarative: it deduplicates events by ID, groups
the resulting events by turn and (turn, operation), then derives values from sets.
It does not replay the production reducer's transitions.

Only valid version-1 events with the fixed UTC timestamp below are generated.
The contract does not determine the following, so this checker leaves them open:

* The message carried by the specified ValueError rejection.
* The values, ordering, and retention rules of schema_version, session_id,
  last_observed_at, and recent_activity in the full reducer result.
* Validation of malformed envelopes, unknown event types, IDs, data fields,
  sources, timestamps, or exit-code types outside the valid inputs used here.
* Whether lifecycle rules beyond the stated historical flags apply to other
  event combinations; no such rule is assumed here.

The reference was written without reading the reducer implementation. The CLI
imports that implementation only when this file is run, after this reference
has been saved.
"""

from __future__ import annotations

import argparse
import copy
import importlib.util
import itertools
import json
from pathlib import Path
from typing import Any, Callable, Iterable


OBSERVED_AT = "2026-09-26T00:00:00Z"
SESSION = "session-1"
TURN = "turn-1"
OPERATION = "operation-1"


class SpecReject(Exception):
    """The stated projection contract rejects this trace."""


def event(
    event_id: str,
    event_type: str,
    *,
    session_id: str = SESSION,
    turn_id: str | None = None,
    operation_id: str | None = None,
    data: dict[str, Any] | None = None,
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "event_id": event_id,
        "session_id": session_id,
        "turn_id": turn_id,
        "operation_id": operation_id,
        "type": event_type,
        "source": "runtime_observer",
        "observed_at": OBSERVED_AT,
        "data": {} if data is None else data,
    }


POOL = (
    event("start-bash", "tool.started", turn_id=TURN, operation_id=OPERATION,
          data={"tool_name": "Bash"}),
    event("finish-null", "tool.finished", turn_id=TURN, operation_id=OPERATION,
          data={"tool_name": "Bash", "exit_code": None}),
    event("finish-zero", "tool.finished", turn_id=TURN, operation_id=OPERATION,
          data={"tool_name": "Bash", "exit_code": 0}),
    event("finish-one", "tool.finished", turn_id=TURN, operation_id=OPERATION,
          data={"tool_name": "Bash", "exit_code": 1}),
    event("start-read", "tool.started", turn_id=TURN, operation_id=OPERATION,
          data={"tool_name": "Read"}),
    event("stop", "turn.stop_requested", turn_id=TURN),
    event("interrupt", "turn.interrupted", turn_id=TURN),
    event("end", "session.ended"),
)


def reference_projection(events: Iterable[dict[str, Any]]) -> dict[str, Any]:
    """Derive the specified projection by grouping facts, independent of order."""
    by_id: dict[str, dict[str, Any]] = {}
    for item in events:
        previous = by_id.get(item["event_id"])
        if previous is not None and previous != item:
            raise SpecReject("one event_id identifies different events")
        by_id[item["event_id"]] = item

    unique = tuple(by_id.values())
    if len({item["session_id"] for item in unique}) > 1:
        raise SpecReject("events belong to different sessions")

    turn_ids = {item["turn_id"] for item in unique if item["turn_id"] is not None}
    turn_ids.update(
        item["turn_id"] for item in unique
        if item["type"] in {"turn.started", "turn.stop_requested", "turn.interrupted"}
    )
    operation_keys = {
        (item["turn_id"], item["operation_id"])
        for item in unique if item["type"] in {"tool.started", "tool.finished"}
    }

    operations_by_turn: dict[str, dict[str, Any]] = {turn_id: {} for turn_id in turn_ids}
    for turn_id, operation_id in operation_keys:
        facts = {
            item["type"] for item in unique
            if item["turn_id"] == turn_id and item["operation_id"] == operation_id
        }
        matching = (
            item for item in unique
            if item["turn_id"] == turn_id and item["operation_id"] == operation_id
        )
        names = {item["data"]["tool_name"] for item in matching}
        concrete_codes = {
            item["data"]["exit_code"] for item in unique
            if item["turn_id"] == turn_id
            and item["operation_id"] == operation_id
            and item["type"] == "tool.finished"
            and item["data"]["exit_code"] is not None
        }
        if len(names) != 1:
            raise SpecReject("one operation has different tool names")
        if len(concrete_codes) > 1:
            raise SpecReject("one operation has different concrete exit codes")
        operations_by_turn[turn_id][operation_id] = {
            "tool_name": next(iter(names)),
            "started": "tool.started" in facts,
            "finished": "tool.finished" in facts,
            "exit_code": next(iter(concrete_codes)) if concrete_codes else None,
        }

    turns = {
        turn_id: {
            "stop_requested": any(
                item["turn_id"] == turn_id and item["type"] == "turn.stop_requested"
                for item in unique
            ),
            "interrupted": any(
                item["turn_id"] == turn_id and item["type"] == "turn.interrupted"
                for item in unique
            ),
            "operations": operations_by_turn[turn_id],
        }
        for turn_id in turn_ids
    }
    return {
        "session_ended": any(item["type"] == "session.ended" for item in unique),
        "turns": turns,
    }


def _projection(result: Any) -> dict[str, Any]:
    if not isinstance(result, dict):
        raise AssertionError("reducer did not return a dictionary")
    missing = {"session_ended", "turns"} - result.keys()
    if missing:
        raise AssertionError(f"reducer omitted projection fields: {sorted(missing)}")
    return {"session_ended": result["session_ended"], "turns": result["turns"]}


def _check_case(
    reducer: Callable[[list[dict[str, Any]]], Any],
    trace: tuple[dict[str, Any], ...],
    label: str,
) -> bool:
    """Return True for acceptance, False for the specified rejection."""
    try:
        expected = reference_projection(trace)
    except SpecReject:
        expected = None

    try:
        # A copy prevents a reducer from changing the reference's event objects.
        actual_result = reducer(copy.deepcopy(list(trace)))
    except Exception as exc:
        if expected is None and isinstance(exc, ValueError):
            return False
        if expected is None:
            raise AssertionError(
                f"{label}: reducer raised {type(exc).__name__} instead of ValueError "
                "for a rejected trace"
            ) from exc
        raise AssertionError(
            f"{label}: reducer rejected an accepted trace: {type(exc).__name__}: {exc}"
        ) from exc

    if expected is None:
        raise AssertionError(f"{label}: reducer accepted a trace the contract rejects")
    actual = _projection(actual_result)
    if actual != expected:
        raise AssertionError(
            f"{label}: projection mismatch; expected={expected!r}, actual={actual!r}"
        )
    return True


def _explicit_cases() -> dict[str, tuple[dict[str, Any], ...]]:
    other_session = event(
        "other-session", "session.ended", session_id="session-2"
    )
    other_turn = event(
        "other-turn", "tool.started", turn_id="turn-2", operation_id=OPERATION,
        data={"tool_name": "Read"},
    )
    other_operation = event(
        "other-operation", "tool.started", turn_id=TURN,
        operation_id="operation-2", data={"tool_name": "Read"},
    )
    conflicting_id = event(
        POOL[0]["event_id"], "tool.started", turn_id=TURN,
        operation_id=OPERATION, data={"tool_name": "Read"},
    )
    many_starts = tuple(
        event(
            f"many-start-{index}", "tool.started", turn_id=TURN,
            operation_id=f"operation-{index + 10}", data={"tool_name": "Bash"},
        )
        for index in range(61)
    )
    return {
        "session isolation": (POOL[7], other_session),
        "same operation ID in different turns": (POOL[0], other_turn),
        "different operations in one turn": (POOL[0], other_operation),
        "conflicting duplicate event ID": (POOL[0], conflicting_id),
        "61 starts retained in projection": many_starts,
    }


def _mutant_empty_projection(
    reducer: Callable[[list[dict[str, Any]]], Any],
) -> Callable[[list[dict[str, Any]]], Any]:
    def mutant(trace: list[dict[str, Any]]) -> dict[str, Any]:
        reducer(trace)
        return {"session_ended": False, "turns": {}}
    return mutant


def _mutant_alias_turn_ids(
    reducer: Callable[[list[dict[str, Any]]], Any],
) -> Callable[[list[dict[str, Any]]], Any]:
    def mutant(trace: list[dict[str, Any]]) -> dict[str, Any]:
        result = copy.deepcopy(reducer(trace))
        turns = result["turns"]
        if "turn-2" in turns:
            turns["turn-1"] = turns.pop("turn-2")
        return result
    return mutant


def _mutant_crash_on_rejection(
    reducer: Callable[[list[dict[str, Any]]], Any],
) -> Callable[[list[dict[str, Any]]], Any]:
    def mutant(trace: list[dict[str, Any]]) -> Any:
        try:
            return reducer(trace)
        except ValueError as exc:
            raise RuntimeError("injected crash on rejected trace") from exc
    return mutant


def _require_mutant_detected(
    mutant: Callable[[list[dict[str, Any]]], Any],
    trace: tuple[dict[str, Any], ...],
    label: str,
) -> None:
    try:
        _check_case(mutant, trace, f"mutation control: {label}")
    except AssertionError:
        return
    raise AssertionError(f"mutation control escaped the checker: {label}")


def run_checks(reducer: Callable[[list[dict[str, Any]]], Any]) -> dict[str, Any]:
    """Check all 4,681 pool traces and targeted cases; return JSON-safe counts."""
    accepted = 0
    rejected = 0
    for length in range(5):
        for indexes in itertools.product(range(len(POOL)), repeat=length):
            trace = tuple(POOL[index] for index in indexes)
            label = "pool indexes " + repr(indexes)
            if _check_case(reducer, trace, label):
                accepted += 1
            else:
                rejected += 1

    explicit = _explicit_cases()
    explicit_accepted = 0
    explicit_rejected = 0
    for label, trace in explicit.items():
        if _check_case(reducer, trace, label):
            explicit_accepted += 1
        else:
            explicit_rejected += 1

    # Each control is a wrapper around the same reducer that just passed.
    _require_mutant_detected(
        _mutant_empty_projection(reducer), (POOL[0],), "empty projection"
    )
    _require_mutant_detected(
        _mutant_alias_turn_ids(reducer),
        explicit["same operation ID in different turns"],
        "aliased turn IDs",
    )
    _require_mutant_detected(
        _mutant_crash_on_rejection(reducer),
        explicit["conflicting duplicate event ID"],
        "RuntimeError on rejected trace",
    )

    return {
        "pool_traces": accepted + rejected,
        "pool_accepted": accepted,
        "pool_rejected": rejected,
        "explicit_cases": len(explicit),
        "explicit_accepted": explicit_accepted,
        "explicit_rejected": explicit_rejected,
        "mutation_controls_detected": [
            "empty projection", "aliased turn IDs", "RuntimeError on rejected trace"
        ],
    }


def _load_reducer(path: Path) -> Callable[[list[dict[str, Any]]], Any]:
    spec = importlib.util.spec_from_file_location("telemetry_under_test", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot import reducer module from {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    reducer = getattr(module, "reduce_events")
    if not callable(reducer):
        raise TypeError("telemetry.reduce_events is not callable")
    return reducer


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--implementation", type=Path,
        default=Path(__file__).resolve().parents[2] / "tools/telemetry/telemetry.py",
        help="module file exporting reduce_events",
    )
    args = parser.parse_args()
    # This is the only route that imports the production module.
    summary = run_checks(_load_reducer(args.implementation))
    print(json.dumps(summary, sort_keys=True))


if __name__ == "__main__":
    main()
