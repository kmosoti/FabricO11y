"""Retain the actual degraded-done bootstrap refusal without extending its wait."""
from pathlib import Path
import os
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

import guest as lifecycle


class Readiness(unittest.TestCase):
    def test_actual_readiness_shell_refuses_a_nonempty_swap_table(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'os-release').write_text('ID=fedora\nVERSION_ID=44\n')
            (root / 'boot-id').write_text('11111111-1111-1111-1111-111111111111\n')
            (root / 'swaps').write_text('Filename Type Size Used Priority\n/dev/zram0 partition 100 0 100\n')
            bins = root / 'bin'
            bins.mkdir()
            for name, body in {'stat': 'echo cgroup2fs', 'getenforce': 'echo Enforcing',
                               'systemctl': 'echo systemd', 'rpm': 'true',
                               'readlink': 'echo /usr/lib/systemd/systemd'}.items():
                path = bins / name
                path.write_text('#!/bin/sh\n' + body + '\n')
                path.chmod(0o700)
            guest = object.__new__(lifecycle.Guest)
            guest.role, guest.family, guest.results, guest.ssh = 'edge', 'fedora', root, ['fixture-ssh']
            guest.child = type('Running', (), {'poll': lambda self: None})()

            def transport(args, *, input_text=None, **kwargs):
                if input_text is None:
                    return subprocess.CompletedProcess(args, 0, 'status: done\n', '')
                # Execute the production shell, substituting only fixture inputs
                # and OS tools. The independent swap fixture must make it refuse.
                script = input_text.replace('/etc/os-release', str(root / 'os-release'))
                script = script.replace('/proc/sys/kernel/random/boot_id', str(root / 'boot-id'))
                script = script.replace('/proc/swaps', str(root / 'swaps'))
                return subprocess.run(['bash', '-s'], input=script, text=True, capture_output=True,
                    env={**os.environ, 'PATH': str(bins) + ':' + os.environ['PATH']}, **kwargs)

            with patch.object(lifecycle.Q, 'run', side_effect=transport):
                with self.assertRaises(subprocess.CalledProcessError):
                    guest.ready(deadline=time.monotonic() + 1)

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
