"""Transport/phase controls; no native guest or owner-acceptance claim."""
import json
import os
from pathlib import Path
import sys
import tempfile
import types
import unittest

from owner_commands import Commands, PHASES
from owner_guide import Block


class CommandTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(dir=os.environ["FABRIC_SCRATCH_ROOT"])
        self.root = Path(self.temporary.name)
        fixture = self.root / "ssh-fixture.py"
        fixture.write_text("import shlex,subprocess,sys\nraise SystemExit(subprocess.call(shlex.split(sys.argv[1])))\n")
        self.guest = types.SimpleNamespace(ssh=[sys.executable, str(fixture)],
                                          boot_id="literal-source-fixture", role="server", family="debian")
        self.commands = Commands(self.root / "evidence")
        self.commands.phase("boot")

    def tearDown(self):
        self.temporary.cleanup()

    def test_registered_allocations_total_1800(self):
        self.assertEqual(sum(seconds for _, seconds in PHASES), 1800)

    def test_phase_omission_refused(self):
        with self.assertRaisesRegex(ValueError, "omitted"):
            self.commands.phase("tls")

    def test_verbatim_shell_quoting_and_private_stdin(self):
        block = Block("docs/operations.md", "literal", 1, 3, b"printf '%s\\n' 'a $HOME `literal`'\ncat\n")
        record = self.commands.shell(self.guest, block, stdin=b"private-test-token\n")
        stdout = Path(record["stdout"]["path"])
        self.assertEqual(stdout.read_bytes(), b"a $HOME `literal`\nprivate-test-token\n")
        self.assertEqual(stdout.stat().st_mode & 0o777, 0o600)
        self.assertNotIn("private-test-token", json.dumps(self.commands.records))
        self.assertEqual(record["exit"], 0)
        self.assertTrue(record["ssh_reaped"])
        self.assertEqual(record["source"]["command"], block.command.decode())

    def test_actual_failure_preserved_and_no_continuation(self):
        block = Block("docs/operations.md", "literal", 1, 3, b"echo literal-failure >&2\nexit 7\n")
        with self.assertRaisesRegex(RuntimeError, "guide command failed"):
            self.commands.shell(self.guest, block)
        record = self.commands.records[-1]
        self.assertEqual(record["exit"], 7)
        self.assertEqual(Path(record["stderr"]["path"]).read_bytes(), b"literal-failure\n")
        self.assertTrue(record["ssh_reaped"])
        with self.assertRaisesRegex(ValueError, "resumed after failure"):
            self.commands.phase("install")

    def test_elapsed_phase_refuses_before_command(self):
        self.commands.deadline_ns = 1
        with self.assertRaises(TimeoutError):
            self.commands.shell(self.guest, Block("docs/operations.md", "literal", 1, 3, b"exit 99\n"))
        self.assertEqual(len(self.commands.records), 1)
        self.assertTrue(self.commands.failed)


if __name__ == "__main__":
    unittest.main()
