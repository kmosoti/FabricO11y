"""Exercise real hook scripts in temporary repositories, including failure paths."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]


class HookContract(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="fabric-hooks-")
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        for directory in (".codex/hooks", "tools/docs", "docs"):
            (self.root / directory).mkdir(parents=True)
        self.hook = self.root / ".codex/hooks/run.py"
        shutil.copyfile(ROOT / ".codex/hooks/run.py", self.hook)
        shutil.copyfile(ROOT / "tools/docs/check.mjs", self.root / "tools/docs/check.mjs")
        (self.root / "tools/docs/node_modules").symlink_to(
            ROOT / "tools/docs/node_modules", target_is_directory=True
        )
        (self.root / "docs/CURRENT.md").write_text("# State\n\nOne example event.\n")
        (self.root / "README.md").write_text("# Demo\n\n[State](docs/CURRENT.md)\n")

    def invoke(self, payload, env=None):
        result = subprocess.run(
            [sys.executable, "-B", str(self.hook)],
            input=json.dumps(payload),
            text=True,
            capture_output=True,
            cwd=self.root / "docs",
            env=env,
            timeout=25,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_session_start_from_subdirectory(self):
        response = self.invoke({"hook_event_name": "SessionStart", "cwd": str(self.root / "docs")})
        output = response["hookSpecificOutput"]
        self.assertEqual(output["hookEventName"], "SessionStart")
        self.assertIn("One example event.", output["additionalContext"])

    def test_context_is_bounded(self):
        (self.root / "docs/CURRENT.md").write_text("x" * 12000)
        context = self.invoke({"hook_event_name": "SessionStart"})["hookSpecificOutput"]["additionalContext"]
        self.assertLess(len(context), 8500)
        self.assertIn("Truncated", context)

    def test_missing_state_is_reported(self):
        (self.root / "docs/CURRENT.md").unlink()
        response = self.invoke({"hook_event_name": "SessionStart"})
        self.assertIn("Cannot load", response["systemMessage"])

    def test_valid_docs_allow_stop(self):
        self.assertEqual(self.invoke({"hook_event_name": "Stop"}), {})

    def test_broken_link_requests_one_continuation(self):
        readme = self.root / "README.md"
        broken = "# Demo\n\n[Missing](missing.md)\n"
        readme.write_text(broken)
        response = self.invoke({"hook_event_name": "Stop", "stop_hook_active": False})
        self.assertEqual(response["decision"], "block")
        self.assertIn("missing.md", response["reason"])
        self.assertEqual(readme.read_text(), broken)

    def test_continuation_guard_reports_remaining_failure(self):
        (self.root / "README.md").write_text("[Missing](missing.md)\n")
        response = self.invoke({"hook_event_name": "Stop", "stop_hook_active": True})
        self.assertNotIn("decision", response)
        self.assertIn("missing.md", response["systemMessage"])

    def test_missing_runtime_is_not_reported_as_success(self):
        env = dict(os.environ, PATH="")
        response = self.invoke({"hook_event_name": "Stop"}, env=env)
        self.assertIn("skipped", response["systemMessage"])
        self.assertNotIn("decision", response)

    def test_missing_dependencies_are_reported(self):
        (self.root / "tools/docs/node_modules").unlink()
        response = self.invoke({"hook_event_name": "Stop"})
        self.assertIn("skipped", response["systemMessage"])

    def test_other_event_is_ignored(self):
        self.assertEqual(self.invoke({"hook_event_name": "PostToolUse"}), {})

    def test_malformed_input_fails_explicitly(self):
        result = subprocess.run(
            [sys.executable, "-B", str(self.hook)],
            input="{broken",
            text=True,
            capture_output=True,
            cwd=self.root,
            timeout=5,
            check=False,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("Invalid hook input", result.stderr)
        self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    if not shutil.which("bun") or not (ROOT / "tools/docs/node_modules").is_dir():
        sys.exit("Install Bun and run bun install --cwd tools/docs --frozen-lockfile first.")
    unittest.main(verbosity=2)
