"""Retain the actual degraded-done bootstrap refusal without extending its wait."""
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

import guest as lifecycle


class Readiness(unittest.TestCase):
    def test_degraded_done_fails_immediately_and_retains_bootstrap_output(self):
        with tempfile.TemporaryDirectory() as directory:
            guest = object.__new__(lifecycle.Guest)
            guest.role, guest.results, guest.ssh = 'edge', Path(directory), ['fixture-ssh']
            guest.child = type('Running', (), {'poll': lambda self: None})()
            result = subprocess.CompletedProcess([], 2,
                'status: done\nextended_status: degraded done\nrecoverable_errors: hostname\n', '')
            with patch.object(lifecycle.Q, 'run', return_value=result), patch.object(lifecycle.time, 'sleep') as sleep:
                with self.assertRaisesRegex(RuntimeError, 'cloud-init completed unsuccessfully'):
                    guest.ready(deadline=time.monotonic() + 1)
            sleep.assert_not_called()
            self.assertIn('exit=2', (Path(directory) / 'edge-bootstrap.txt').read_text())


if __name__ == '__main__':
    unittest.main()
