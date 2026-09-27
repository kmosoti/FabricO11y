"""Independent, specification-only oracle for passive telemetry v1.

Run: TELEMETRY_MODULE=/absolute/path/telemetry.py python3 -B test_contract.py

Open questions deliberately not asserted here:
* Which valid ISO 8601 spellings beyond ordinary offset timestamps and Z are accepted?
* Which UUID version or string representation should generated event IDs use?
* What lock-file layout and JSON whitespace/key order should journals use?
* Are unsupported non-string hook names equivalent to unsupported string names?
* How should a malformed observed_at be handled for an unsupported hook?
"""

import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import uuid


MODULE_PATH = os.environ.get("TELEMETRY_MODULE", str(Path(__file__).with_name("telemetry.py")))
if not MODULE_PATH or not Path(MODULE_PATH).is_absolute():
    raise RuntimeError("TELEMETRY_MODULE must be an absolute file path")
spec = importlib.util.spec_from_file_location("telemetry_under_test", MODULE_PATH)
telemetry = importlib.util.module_from_spec(spec)
spec.loader.exec_module(telemetry)

AT = "2026-01-02T03:04:05Z"
ENVELOPE = {
    "schema_version", "event_id", "session_id", "turn_id", "operation_id",
    "type", "source", "observed_at", "data",
}


def event(kind="session.started", *, eid="event-1", session="session /../ Ω",
          turn=None, operation=None, data=None, at=AT):
    if data is None:
        data = {}
    return {
        "schema_version": 1, "event_id": eid, "session_id": session,
        "turn_id": turn, "operation_id": operation, "type": kind,
        "source": "runtime_observer", "observed_at": at, "data": data,
    }


def tool(kind, *, eid, turn="turn-1", operation="op-1", name="Bash",
         exit_code=None, session="session-1", at=AT):
    data = {"tool_name": name}
    if kind == "tool.finished":
        data["exit_code"] = exit_code
    return event(kind, eid=eid, session=session, turn=turn,
                 operation=operation, data=data, at=at)


class ValidationTests(unittest.TestCase):
    def test_all_variants_and_optional_exit_code(self):
        good = [
            event(), event("session.ended"),
            event("turn.started", turn="t"),
            event("turn.stop_requested", turn="t"),
            event("turn.interrupted", turn="t"),
            tool("tool.started", eid="ts"),
            tool("tool.finished", eid="tf", exit_code=None),
            tool("tool.finished", eid="tf2", exit_code=-7),
        ]
        for item in good:
            with self.subTest(item=item):
                self.assertIsNone(telemetry.validate_event(item))

    def test_exact_envelope_and_field_types(self):
        base = event()
        bad = []
        for key in ENVELOPE:
            item = copy.deepcopy(base)
            del item[key]
            bad.append(item)
        item = copy.deepcopy(base)
        item["raw_prompt"] = "secret"
        bad.append(item)
        for key, value in (
            ("schema_version", True), ("schema_version", 1.0),
            ("schema_version", 2), ("source", "untrusted"),
            ("type", "annotation"), ("event_id", ""),
            ("event_id", 123), ("event_id", "x" * 257),
            ("session_id", ""), ("session_id", None),
            ("session_id", "x" * 257), ("turn_id", ""),
            ("operation_id", ""), ("observed_at", 1),
            ("observed_at", "x" * 65),
            ("observed_at", "2026-01-02T03:04:05"),
            ("observed_at", "2026-02-30T03:04:05Z"),
            ("data", []),
        ):
            item = copy.deepcopy(base)
            item[key] = value
            bad.append(item)
        for item in bad:
            with self.subTest(item=item):
                with self.assertRaises(ValueError):
                    telemetry.validate_event(item)

    def test_variant_id_and_data_rules(self):
        bad = [
            event("session.started", turn="t"),
            event("session.ended", operation="o"),
            event("turn.started"),
            event("turn.stop_requested", turn="t", operation="o"),
            event("turn.interrupted", turn="t", data={"reason": "secret"}),
            event("tool.started", turn="t", data={"tool_name": "Bash"}),
            event("tool.started", turn="t", operation="o"),
            event("tool.started", turn="t", operation="o", data={"tool_name": ""}),
            event("tool.started", turn="t", operation="o", data={"tool_name": "x" * 129}),
            event("tool.started", turn="t", operation="o", data={"tool_name": "Bash", "arguments": "secret"}),
            event("tool.finished", turn="t", operation="o", data={"tool_name": "Bash"}),
            event("tool.finished", turn="t", operation="o", data={"tool_name": "Bash", "exit_code": True}),
            event("tool.finished", turn="t", operation="o", data={"tool_name": "Bash", "exit_code": 0.0}),
            event("tool.finished", turn="t", operation="o", data={"tool_name": "Bash", "exit_code": "0"}),
            event("tool.finished", turn="t", operation="o", data={"tool_name": "Bash", "exit_code": 0, "output": "secret"}),
        ]
        for item in bad:
            with self.subTest(item=item):
                with self.assertRaises(ValueError):
                    telemetry.validate_event(item)

    def test_schema_declares_draft_2020_12(self):
        schema = telemetry.event_schema()
        self.assertIsInstance(schema, dict)
        self.assertIn("2020-12", schema.get("$schema", ""))
        json.dumps(schema)


class NormalizationTests(unittest.TestCase):
    def payload(self, name, **overrides):
        value = {
            "hook_event_name": name, "session_id": "session-1",
            "turn_id": "turn-1", "tool_use_id": "op-1",
            "tool_name": "Bash", "prompt": "DO NOT STORE",
            "tool_input": {"arguments": "DO NOT STORE"},
            "transcript": "DO NOT STORE", "output": "DO NOT STORE",
        }
        value.update(overrides)
        return value

    def test_supported_hooks_and_raw_field_elision(self):
        mappings = {
            "SessionStart": "session.started", "SessionEnd": "session.ended",
            "UserPromptSubmit": "turn.started", "Stop": "turn.stop_requested",
            "Interrupt": "turn.interrupted", "PreToolUse": "tool.started",
            "PostToolUse": "tool.finished",
        }
        for hook, kind in mappings.items():
            with self.subTest(hook=hook):
                result = telemetry.normalize_hook(self.payload(hook), AT)
                self.assertIsNone(telemetry.validate_event(result))
                self.assertEqual(result["type"], kind)
                self.assertEqual(result["observed_at"], AT)
                self.assertEqual(result["source"], "runtime_observer")
                self.assertEqual(set(result), ENVELOPE)
                uuid.UUID(result["event_id"])
                if hook.startswith("Session"):
                    self.assertIsNone(result["turn_id"])
                    self.assertIsNone(result["operation_id"])
                elif hook in ("PreToolUse", "PostToolUse"):
                    self.assertEqual(result["operation_id"], "op-1")
                else:
                    self.assertEqual(result["turn_id"], "turn-1")
                    self.assertIsNone(result["operation_id"])
                self.assertNotIn("DO NOT STORE", json.dumps(result))
        self.assertNotEqual(
            telemetry.normalize_hook(self.payload("SessionStart"), AT)["event_id"],
            telemetry.normalize_hook(self.payload("SessionStart"), AT)["event_id"],
        )

    def test_unsupported_and_malformed_supported(self):
        self.assertIsNone(telemetry.normalize_hook({}, AT))
        self.assertIsNone(telemetry.normalize_hook(self.payload("FutureHook"), AT))
        with self.assertRaises(ValueError):
            telemetry.normalize_hook([], AT)
        for hook, missing in (
            ("SessionStart", "session_id"),
            ("UserPromptSubmit", "turn_id"),
            ("Stop", "turn_id"),
            ("Interrupt", "turn_id"),
            ("PreToolUse", "tool_use_id"),
            ("PreToolUse", "tool_name"),
            ("PostToolUse", "tool_use_id"),
            ("PostToolUse", "tool_name"),
        ):
            item = self.payload(hook)
            del item[missing]
            with self.subTest(hook=hook, missing=missing):
                with self.assertRaises(ValueError):
                    telemetry.normalize_hook(item, AT)
        for name in (None, 1, False):
            with self.subTest(name=name):
                with self.assertRaises(ValueError):
                    telemetry.normalize_hook(self.payload("PreToolUse", tool_name=name), AT)

    def test_exit_code_only_direct_bash_integer(self):
        cases = [
            ({"exit_code": -3}, "Bash", -3),
            ({"exit_code": True}, "Bash", None),
            ({"exit_code": "1"}, "Bash", None),
            ({"result": {"exit_code": 9}}, "Bash", None),
            ({"output": "exit_code: 9"}, "Bash", None),
            ("exit_code: 9", "Bash", None),
            ({"exit_code": 0}, "Read", None),
        ]
        for response, name, expected in cases:
            with self.subTest(response=response, name=name):
                result = telemetry.normalize_hook(
                    self.payload("PostToolUse", tool_name=name,
                                 tool_response=response), AT)
                self.assertEqual(result["data"],
                                 {"tool_name": name, "exit_code": expected})
        result = telemetry.normalize_hook(self.payload("PostToolUse"), AT)
        self.assertIsNone(result["data"]["exit_code"])


class JournalTests(unittest.TestCase):
    def test_append_path_hash_and_round_trip(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "journal"
            item = event(session="../../unsafe/Ω")
            path = telemetry.append_event(root, item)
            expected = hashlib.sha256(item["session_id"].encode("utf-8")).hexdigest() + ".jsonl"
            self.assertEqual(path, root / expected)
            self.assertEqual(telemetry.read_events(path), [item])
            self.assertTrue(path.read_bytes().endswith(b"\n"))
            self.assertEqual(len(path.read_bytes().splitlines()), 1)

    def test_invalid_event_rejected_before_root_creation(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "new-root"
            bad = event()
            bad["prompt"] = "secret"
            with self.assertRaises(ValueError):
                telemetry.append_event(root, bad)
            self.assertFalse(root.exists())

    def test_read_empty_missing_malformed_truncated_mixed_and_large_line(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "journal.jsonl"
            with self.assertRaises(FileNotFoundError):
                telemetry.read_events(path)
            path.write_bytes(b"")
            self.assertEqual(telemetry.read_events(path), [])
            fixtures = [
                b"{\n", b"not-json\n",
                json.dumps(event()).encode("utf-8"),
                json.dumps({**event(), "prompt": "secret"}).encode("utf-8") + b"\n",
                json.dumps(event()).encode("utf-8") + b"\n" +
                json.dumps(event(eid="other", session="different")).encode("utf-8") + b"\n",
                b"x" * 8193 + b"\n",
                b"\xff\n",
            ]
            for raw in fixtures:
                with self.subTest(raw=raw[:40]):
                    path.write_bytes(raw)
                    with self.assertRaises(ValueError):
                        telemetry.read_events(path)

    def test_concurrent_subprocess_appends_are_complete(self):
        code = r'''
import importlib.util, os, pathlib, sys
spec = importlib.util.spec_from_file_location("candidate", os.environ["TELEMETRY_MODULE"])
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
root = pathlib.Path(sys.argv[1])
worker = sys.argv[2]
for i in range(30):
    mod.append_event(root, {"schema_version": 1, "event_id": worker + "-" + str(i),
        "session_id": "shared-session", "turn_id": None, "operation_id": None,
        "type": "session.started", "source": "runtime_observer",
        "observed_at": "2026-01-02T03:04:05Z", "data": {}})
'''
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "root"
            workers = [subprocess.Popen(
                [sys.executable, "-B", "-c", code, str(root), str(i)],
                env={**os.environ, "TELEMETRY_MODULE": MODULE_PATH},
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
            ) for i in range(4)]
            for proc in workers:
                out, err = proc.communicate(timeout=20)
                self.assertEqual(proc.returncode, 0, (out, err))
            path = root / (hashlib.sha256(b"shared-session").hexdigest() + ".jsonl")
            values = telemetry.read_events(path)
            self.assertEqual(len(values), 120)
            self.assertEqual(len({v["event_id"] for v in values}), 120)


class ReductionTests(unittest.TestCase):
    def test_empty_exact_shape(self):
        self.assertEqual(telemetry.reduce_events([]), {
            "schema_version": 1, "session_id": None, "session_ended": False,
            "last_observed_at": None, "turns": {}, "recent_activity": [],
        })

    def test_reordered_trace_monotonic_flags_and_chronological_max(self):
        values = [
            tool("tool.finished", eid="finish", exit_code=7,
                 at="2026-01-02T09:00:00+02:00"),
            event("turn.stop_requested", eid="stop", session="session-1", turn="turn-1"),
            event("session.ended", eid="end", session="session-1",
                  at="2026-01-02T06:00:00Z"),
            tool("tool.started", eid="start", at="2026-01-02T07:30:00Z"),
            event("turn.interrupted", eid="interrupt", session="session-1", turn="turn-1"),
            event("session.started", eid="session-start", session="session-1"),
        ]
        state = telemetry.reduce_events(values)
        self.assertEqual(set(state), {"schema_version", "session_id", "session_ended",
                                      "last_observed_at", "turns", "recent_activity"})
        self.assertEqual(state["session_id"], "session-1")
        self.assertTrue(state["session_ended"])
        self.assertEqual(state["last_observed_at"], "2026-01-02T07:30:00Z")
        self.assertEqual(state["recent_activity"], values)
        self.assertEqual(state["turns"], {"turn-1": {
            "stop_requested": True, "interrupted": True,
            "operations": {"op-1": {"tool_name": "Bash", "started": True,
                                     "finished": True, "exit_code": 7}},
        }})

    def test_deduplication_recent_activity_window_and_conflicts(self):
        values = [event(eid=str(i), session="s") for i in range(52)]
        result = telemetry.reduce_events(values + [copy.deepcopy(values[4])])
        self.assertEqual(result["recent_activity"], values[-50:])
        self.assertEqual(result["session_id"], "s")
        conflict = copy.deepcopy(values[4])
        conflict["observed_at"] = "2026-01-03T03:04:05Z"
        with self.assertRaises(ValueError):
            telemetry.reduce_events([values[4], conflict])
        with self.assertRaises(ValueError):
            telemetry.reduce_events([event(session="s"), event(eid="other", session="t")])

    def test_overlapping_operations_and_conflicting_outcomes(self):
        values = [
            tool("tool.started", eid="a-start", operation="a", name="Bash"),
            tool("tool.finished", eid="b-finish", operation="b", name="Read"),
            tool("tool.finished", eid="a-finish", operation="a", name="Bash", exit_code=1),
            tool("tool.started", eid="b-start", operation="b", name="Read"),
            tool("tool.finished", eid="a-again", operation="a", name="Bash", exit_code=1),
        ]
        result = telemetry.reduce_events(values)
        ops = result["turns"]["turn-1"]["operations"]
        self.assertEqual(ops["a"], {"tool_name": "Bash", "started": True,
                                    "finished": True, "exit_code": 1})
        self.assertEqual(ops["b"], {"tool_name": "Read", "started": True,
                                    "finished": True, "exit_code": None})
        for conflict in (
            tool("tool.finished", eid="conflict", operation="a", name="Bash", exit_code=2),
            tool("tool.finished", eid="conflict", operation="a", name="Read", exit_code=1),
            tool("tool.started", eid="conflict", operation="a", name="Read"),
        ):
            with self.subTest(conflict=conflict):
                with self.assertRaises(ValueError):
                    telemetry.reduce_events(values + [conflict])

    def test_unknown_exit_code_preserves_known_result_in_either_order(self):
        for first, second in ((None, 0), (0, None), (None, -3), (-3, None)):
            with self.subTest(first=first, second=second):
                values = [
                    tool("tool.finished", eid="first", exit_code=first),
                    tool("tool.started", eid="start"),
                    tool("tool.finished", eid="second", exit_code=second),
                    tool("tool.finished", eid="unknown-again", exit_code=None),
                ]
                state = telemetry.reduce_events(values)
                operation = state["turns"]["turn-1"]["operations"]["op-1"]
                self.assertEqual(operation, {
                    "tool_name": "Bash", "started": True, "finished": True,
                    "exit_code": first if first is not None else second,
                })
                self.assertEqual(state["recent_activity"], values)


if __name__ == "__main__":
    unittest.main()
