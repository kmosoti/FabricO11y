"""Cross-check the contract using an independent JSON Schema validator.

Install requirements-test.txt in an isolated environment before running.
"""

import copy
import json
import sys

from jsonschema import Draft202012Validator, FormatChecker

import telemetry


BASE = {
    "schema_version": 1, "event_id": "e", "session_id": "s",
    "turn_id": None, "operation_id": None, "type": "session.started",
    "source": "runtime_observer", "observed_at": "2026-09-26T12:00:00Z", "data": {},
}


def corpus():
    cases = []
    for kind in ("session.started", "session.ended", "turn.started", "turn.stop_requested",
                 "turn.interrupted", "tool.started", "tool.finished"):
        event = copy.deepcopy(BASE)
        event["type"] = kind
        if not kind.startswith("session."):
            event["turn_id"] = "t"
        if kind.startswith("tool."):
            event["operation_id"] = "o"
            event["data"] = {"tool_name": "Bash"}
        if kind == "tool.finished":
            event["data"]["exit_code"] = 7
        cases.append((event, True, True))
        for field in event:
            bad = copy.deepcopy(event)
            del bad[field]
            cases.append((bad, False, False))
        bad = copy.deepcopy(event)
        bad["data"]["unrecognized"] = "x"
        cases.append((bad, False, False))
    for key, value in (
        ("schema_version", True), ("source", "agent_annotation"),
        ("operation_id", "no"), ("turn_id", "no"),
        ("event_id", ""), ("event_id", "e" * 257),
        ("observed_at", "2026-09-26T12:00:00+01:60"),
        ("observed_at", "2026-09-26 12:00:00Z"),
        ("observed_at", "2026-02-30T12:00:00Z"),
        ("observed_at", "2026-09-26T12:00:00"),
        ("observed_at", "2026-09-26T12:00:00+24:00"),
    ):
        bad = copy.deepcopy(BASE)
        bad[key] = value
        cases.append((bad, False, False))
    # JSON Schema models mathematical integers, not Python's lexical int/float
    # distinction. The recorder deliberately applies the stricter runtime rule.
    version_float = copy.deepcopy(BASE)
    version_float["schema_version"] = 1.0
    cases.append((version_float, False, True))
    exit_float = copy.deepcopy(BASE)
    exit_float.update(type="tool.finished", turn_id="t", operation_id="o",
                      data={"tool_name": "Bash", "exit_code": 1.0})
    cases.append((exit_float, False, True))
    return cases


def main():
    schema = telemetry.event_schema()
    Draft202012Validator.check_schema(schema)
    validator = Draft202012Validator(schema, format_checker=FormatChecker())
    cases = corpus()
    failures = []
    for event, expected_runtime, expected_schema in cases:
        try:
            telemetry.validate_event(event)
            runtime_valid = True
        except ValueError:
            runtime_valid = False
        schema_valid = validator.is_valid(event)
        if runtime_valid != expected_runtime or schema_valid != expected_schema:
            failures.append({"event": event, "expected_runtime": expected_runtime,
                             "expected_schema": expected_schema,
                             "runtime": runtime_valid, "schema": schema_valid})
    print(json.dumps({"cases": len(cases), "failures": failures}, indent=2))
    return int(bool(failures))


if __name__ == "__main__":
    sys.exit(main())
