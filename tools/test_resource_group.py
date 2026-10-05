"""Containment negative controls and child inheritance, without heavy loads."""
import subprocess
import sys
import unittest
import os
import tempfile
from unittest.mock import patch

import resource_group


class Containment(unittest.TestCase):
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
