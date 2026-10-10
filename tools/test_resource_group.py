"""Containment negative controls and child inheritance, without heavy loads."""
import subprocess
import sys
import unittest
import os
import tempfile
from unittest.mock import patch

import resource_group


class Containment(unittest.TestCase):
    def disconnected_wrapper(self, stop_status):
        """CX: self-observation-fast-02 lost its bus; wrapper exited zero."""
        with tempfile.TemporaryDirectory() as directory:
            root = resource_group.Path(directory)
            with patch.object(resource_group, 'STORAGE', root), \
                 patch.object(sys, 'argv', ['resource_group.py', '--', 'true']), \
                 patch.object(resource_group.subprocess, 'call', return_value=0), \
                 patch.object(resource_group.subprocess, 'run',
                              return_value=subprocess.CompletedProcess([], stop_status)):
                status = resource_group.main()
            scratch = list((root / 'scratch').iterdir())
            evidence = list((root / 'evidence').iterdir()) if (root / 'evidence').exists() else []
            return status, len(scratch), len(evidence)

    def test_wrapper_success_without_command_completion_is_incomplete(self):
        self.assertEqual(self.disconnected_wrapper(0), (2, 0, 1))

    def test_unconfirmed_stop_preserves_scratch_in_place(self):
        self.assertEqual(self.disconnected_wrapper(1), (2, 1, 0))

    def test_deadline_default_and_explicit_finite_extension(self):
        self.assertEqual(resource_group.command_options(['--','true']), (1800,False,['true']))
        self.assertEqual(resource_group.command_options(['--runtime-seconds','7500','--','true']), (7500,False,['true']))
        self.assertEqual(resource_group.command_options(['--delegate','--','true']), (1800,True,['true']))
        for value in ('0','-1','16001','inf','nan'):
            with self.assertRaises(ValueError):
                resource_group.command_options(['--runtime-seconds',value,'--','true'])

    def test_real_group_and_child_inherit(self):
        resource_group.require_limits()
        child = subprocess.check_output(
            [sys.executable, '-c',
             "from pathlib import Path; print(Path('/proc/self/cgroup').read_text(), end='')"],
            text=True)
        self.assertEqual(child, resource_group.Path('/proc/self/cgroup').read_text())

    def test_temporary_files_and_builds_use_data_drive(self):
        self.assertTrue(resource_group.DATA_DRIVE.is_mount())
        root = resource_group.STORAGE.resolve()
        self.assertTrue(resource_group.Path(os.environ['CARGO_TARGET_DIR']).is_relative_to(root))
        with tempfile.TemporaryDirectory() as directory:
            self.assertTrue(resource_group.Path(directory).resolve().is_relative_to(root))
            (resource_group.Path(directory) / 'fixture').write_text('owned scratch')
        self.assertFalse(resource_group.Path(directory).exists())

    def test_unlimited_memory_is_rejected(self):
        with patch.object(resource_group.Path, 'read_text',
                          side_effect=['0::/fake\n', 'max\n', '0\n']):
            with self.assertRaisesRegex(RuntimeError, 'unsafe cgroup'):
                resource_group.require_limits()

    def test_swap_is_rejected(self):
        with patch.object(resource_group.Path, 'read_text',
                          side_effect=['0::/fake\n', '8589934592\n', 'max\n']):
            with self.assertRaisesRegex(RuntimeError, 'unsafe cgroup'):
                resource_group.require_limits()

    def test_memory_above_owner_budget_is_rejected(self):
        with patch.object(resource_group.Path, 'read_text',
                          side_effect=['0::/fake\n', str(21 * 1024**3), '0\n']):
            with self.assertRaisesRegex(RuntimeError, 'unsafe cgroup'):
                resource_group.require_limits()

    def test_legacy_cgroup_is_rejected(self):
        with patch.object(resource_group.Path, 'read_text', return_value='1:memory:/fake\n'):
            with self.assertRaisesRegex(RuntimeError, 'cgroup v2'):
                resource_group.require_limits()


if __name__ == '__main__':
    unittest.main()
