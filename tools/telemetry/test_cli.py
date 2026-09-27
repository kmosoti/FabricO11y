"""End-to-end observer checks: silence, explicit read failures, isolated storage."""

import hashlib
import fcntl
import itertools
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

import telemetry

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]


class CliContract(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="fabric-telemetry-cli-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / "events"

    def call(self, *args, raw=None):
        return subprocess.run(
            [sys.executable, "-B", str(HERE / "cli.py"), *args],
            input=raw, text=True, capture_output=True, timeout=5,
        )

    def observe(self, payload):
        raw = payload if isinstance(payload, str) else json.dumps(payload)
        result = self.call("observe", "--root", str(self.root), raw=raw)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""))

    def test_observer_is_silent_and_drops_content(self):
        secret = "PRIVATE_BODY_NOT_TELEMETRY"
        self.observe({"hook_event_name": "UserPromptSubmit", "session_id": "demo", "turn_id": "t", "prompt": secret})
        self.observe({"hook_event_name": "PostToolUse", "session_id": "demo", "turn_id": "t",
                      "tool_use_id": "cmd", "tool_name": "Bash",
                      "tool_input": {"command": secret},
                      "tool_response": {"exit_code": 7, "output": secret}})
        result = self.call("snapshot", "--root", str(self.root), "--session", "demo")
        self.assertEqual(result.returncode, 0, result.stderr)
        state = json.loads(result.stdout)
        op = state["turns"]["t"]["operations"]["cmd"]
        self.assertTrue(op["finished"])
        self.assertEqual(op["exit_code"], 7)
        self.assertNotIn(secret, "".join(p.read_text() for p in self.root.iterdir()))
        self.assertNotIn("complete", result.stdout)
        self.assertNotIn("checks_passed", result.stdout)

    def test_bad_input_and_unwritable_store_never_feed_back(self):
        self.observe('{"prompt":"PRIVATE_BODY", broken')
        self.assertNotIn("PRIVATE_BODY", (self.root / "observer-errors.log").read_text())
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "file"
            path.write_text("not a directory")
            result = self.call("observe", "--root", str(path), raw=json.dumps({
                "hook_event_name": "SessionStart", "session_id": "demo"}))
            self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""))

    def test_duplicate_keys_and_oversized_payload_rejected(self):
        self.observe('{"hook_event_name":"SessionStart","session_id":"a","session_id":"b"}')
        self.observe(" " * 1_048_577)
        self.assertEqual(list(self.root.glob("*.jsonl")), [])
        self.assertEqual(len((self.root / "observer-errors.log").read_text().splitlines()), 2)

    def test_unsupported_hook_has_no_side_effect(self):
        self.observe({"hook_event_name": "FutureUnknownHook", "prompt": "text"})
        self.assertFalse(self.root.exists())

    def test_missing_or_corrupt_session_is_explicit(self):
        result = self.call("snapshot", "--root", str(self.root), "--session", "missing")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "")
        self.observe({"hook_event_name": "SessionStart", "session_id": "demo"})
        path = self.root / (hashlib.sha256(b"demo").hexdigest() + ".jsonl")
        with path.open("a") as stream:
            stream.write('{"incomplete":')
        result = self.call("snapshot", "--root", str(self.root), "--session", "demo")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "")
        self.assertIn("Telemetry read failed", result.stderr)

    def test_checked_in_schema_matches_runtime(self):
        result = self.call("schema")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), telemetry.event_schema())
        self.assertEqual(json.loads((HERE / "event.schema.json").read_text()), telemetry.event_schema())

    def test_configured_commands_work_from_subdirectory(self):
        checkout = Path(self.tmp.name) / "checkout with spaces"
        target = checkout / "tools" / "telemetry"
        target.mkdir(parents=True)
        for name in ("cli.py", "telemetry.py"):
            shutil.copyfile(HERE / name, target / name)
        subprocess.run(["git", "init", "--quiet", str(checkout)], check=True, capture_output=True)
        config = json.loads((REPO / ".codex" / "hooks.json").read_text())
        names = {"SessionStart", "SessionEnd", "UserPromptSubmit", "Stop", "Interrupt", "PreToolUse", "PostToolUse"}
        for name in names:
            handlers = [handler for group in config["hooks"][name] for handler in group["hooks"]
                        if "tools/telemetry/cli.py" in handler["command"]]
            self.assertEqual(len(handlers), 1)
            handler = handlers[0]
            self.assertNotIn("additionalContextLimit", handler)
            self.assertNotIn("statusMessage", handler)
            if name == "SessionEnd":
                self.assertFalse(handler.get("async", False))
            payload = {"hook_event_name": name, "session_id": "configured", "turn_id": "t",
                       "tool_use_id": "o", "tool_name": "Bash", "tool_response": {"exit_code": 0}}
            result = subprocess.run(handler["command"], shell=True, cwd=target,
                                    input=json.dumps(payload), text=True, capture_output=True, timeout=5)
            self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""))
        store = checkout / ".local" / "codex-telemetry"
        journals = list(store.glob("*.jsonl"))
        self.assertEqual(len(journals), 1)
        events = telemetry.read_events(journals[0])
        self.assertEqual(len(events), len(names))
        self.assertEqual(telemetry.reduce_events(events)["session_id"], "configured")

    def test_contention_is_silent_for_writer_and_explicit_for_reader(self):
        self.observe({"hook_event_name": "SessionStart", "session_id": "locked"})
        locks = list(self.root.glob("*.lock"))
        self.assertEqual(len(locks), 1)
        with locks[0].open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            self.observe({"hook_event_name": "SessionEnd", "session_id": "locked"})
            result = self.call("snapshot", "--root", str(self.root), "--session", "locked")
            self.assertEqual(result.returncode, 1)
            self.assertEqual(result.stdout, "")
        self.assertIn("TimeoutError", (self.root / "observer-errors.log").read_text())
        result = self.call("snapshot", "--root", str(self.root), "--session", "locked")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(json.loads(result.stdout)["session_ended"])

    def test_all_small_trace_permutations_preserve_observations(self):
        at = "2026-09-26T00:00:00Z"
        payload = {"session_id": "s", "turn_id": "t", "tool_use_id": "o", "tool_name": "Bash"}
        events = [telemetry.normalize_hook({**payload, "hook_event_name": name}, at)
                  for name in ("UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop", "Interrupt")]
        reference = telemetry.reduce_events(events)["turns"]
        for order in itertools.permutations(events):
            self.assertEqual(telemetry.reduce_events(order)["turns"], reference)

    def test_invalid_offset_and_duplicate_journal_key_fail(self):
        for at in ("2026-09-26T12:00:00+01:60", "2026-09-26T12:00:00-24:00"):
            with self.assertRaises(ValueError):
                telemetry.normalize_hook({"hook_event_name": "SessionStart", "session_id": "s"}, at)
        self.observe({"hook_event_name": "SessionStart", "session_id": "s"})
        journal = next(self.root.glob("*.jsonl"))
        original = journal.read_text()
        journal.write_text(original.replace('"schema_version":1', '"schema_version":0,"schema_version":1'))
        self.assertNotEqual(original, journal.read_text())
        with self.assertRaises(ValueError):
            telemetry.read_events(journal)

    def test_fine_timestamp_precision_and_session_filename_mismatch(self):
        values = [telemetry.normalize_hook({"hook_event_name": "SessionStart", "session_id": "bob"}, at)
                  for at in ("2026-09-26T00:00:00.0000001Z", "2026-09-26T00:00:00.0000002Z")]
        for order in (values, list(reversed(values))):
            self.assertEqual(telemetry.reduce_events(order)["last_observed_at"], values[1]["observed_at"])
        journal = telemetry.append_event(self.root, values[0])
        journal.rename(self.root / (hashlib.sha256(b"alice").hexdigest() + ".jsonl"))
        result = self.call("snapshot", "--root", str(self.root), "--session", "alice")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "")
        self.assertIn("identity", result.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
