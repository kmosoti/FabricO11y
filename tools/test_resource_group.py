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
            stop = ({'side_effect': stop_status} if isinstance(stop_status, OSError)
                    else {'return_value': subprocess.CompletedProcess([], stop_status)})
            with patch.object(resource_group, 'STORAGE', root), \
                 patch.object(resource_group, 'RECEIPTS', root / 'receipts'), \
                 patch.object(sys, 'argv', ['resource_group.py', '--', 'true']), \
                 patch.object(resource_group.subprocess, 'call', return_value=0), \
                 patch.object(resource_group.subprocess, 'run', **stop):
                status = resource_group.main()
            receipts = list((root / 'receipts').iterdir())
            self.assertEqual(len(receipts), 1)
            recorded = resource_group.json.loads(receipts[0].read_text())
            self.assertEqual(recorded['exit'], status)
            self.assertEqual(recorded['stop_confirmed'], stop_status == 0)
            if isinstance(stop_status, OSError):
                self.assertEqual(recorded['stop_error'], str(stop_status))
            scratch = list((root / 'scratch').iterdir())
            evidence = list((root / 'evidence').iterdir()) if (root / 'evidence').exists() else []
            return status, len(scratch), len(evidence)

    def test_wrapper_success_without_command_completion_is_incomplete(self):
        self.assertEqual(self.disconnected_wrapper(0), (2, 0, 1))

    def test_unconfirmed_stop_preserves_scratch_in_place(self):
        self.assertEqual(self.disconnected_wrapper(1), (2, 1, 0))

    def test_missing_stop_command_preserves_scratch_and_returns_incomplete(self):
        self.assertEqual(self.disconnected_wrapper(OSError('bus unavailable')), (2, 1, 0))

    def test_group_cleanup_requires_empty_or_removed_group(self):
        receipt = {'cgroup': '/sys/fs/cgroup/fabric-work-test.service'}
        for contents, expected in [('populated 0\nfrozen 0\n', True),
                                   ('populated 1\nfrozen 0\n', False)]:
            with patch.object(resource_group.Path, 'read_text', return_value=contents):
                self.assertEqual(resource_group.group_stopped(receipt, 1), expected)
        for exists in (True, False):
            with patch.object(resource_group.Path, 'read_text', side_effect=FileNotFoundError), \
                 patch.object(resource_group.Path, 'exists', return_value=exists):
                self.assertEqual(resource_group.group_stopped(receipt, 1), not exists)
        self.assertFalse(resource_group.group_stopped(None, 1))

    def test_completion_requires_matching_transport_and_real_integer_exit(self):
        for wrapper, receipt in [
            (0, None), (0, {'state': 'started'}),
            (0, {'state': 'completed', 'exit': True}),
            (0, {'state': 'completed', 'exit': 1}),
            (1, {'state': 'completed', 'exit': 0}),
        ]:
            self.assertEqual(resource_group.completed_status(wrapper, receipt), 2)
        for status in (0, 1, 143):
            self.assertEqual(resource_group.completed_status(
                status, {'state': 'completed', 'exit': status}), status)

    def test_completion_receipt_rejects_malformed_or_unrelated_runs(self):
        with tempfile.TemporaryDirectory() as directory:
            path = resource_group.Path(directory) / 'command.json'
            receipt = {'unit': 'fabric-work-test', 'command': ['true'],
                       'cgroup': '/sys/fs/cgroup/fabric-work-test.service'}
            resource_group.write_command_receipt(path, receipt)
            self.assertEqual(resource_group.read_command_receipt(
                path, 'fabric-work-test', ['true']), receipt)
            self.assertIsNone(resource_group.read_command_receipt(path, 'other', ['true']))
            self.assertIsNone(resource_group.read_command_receipt(
                path, 'fabric-work-test', ['false']))
            for value in ('{', '[]', 'null'):
                path.write_text(value)
                self.assertIsNone(resource_group.read_command_receipt(
                    path, 'fabric-work-test', ['true']))

    def test_inner_supervisor_records_actual_nonzero_command_exit(self):
        with tempfile.TemporaryDirectory() as directory:
            path = resource_group.Path(directory) / 'command.json'
            command = [sys.executable, '-c', 'raise SystemExit(7)']
            group = resource_group.require_limits()
            unit = group.name.removesuffix('.service')
            self.assertEqual(resource_group.run_inside([unit, str(path), *command]), 7)
            receipt = resource_group.read_command_receipt(path, unit, command)
            self.assertEqual(receipt['state'], 'completed')
            self.assertEqual(receipt['exit'], 7)
            self.assertEqual(resource_group.completed_status(7, receipt), 7)
            self.assertFalse(path.with_suffix('.pending').exists())

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
