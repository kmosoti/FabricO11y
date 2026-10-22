"""Containment negative controls and child inheritance, without heavy loads."""
import subprocess
import sys
import unittest
import os
import tempfile
from unittest.mock import patch

import resource_group

GIB = 1024**3


class Containment(unittest.TestCase):
    def test_release_profile_rejects_each_unenforced_boundary(self):
        with tempfile.TemporaryDirectory() as directory:
            group = resource_group.Path(directory)
            values = {'memory.high': str(1706 * 1024**2),
                      'memory.max': str(2048 * 1024**2),
                      'memory.swap.max': '0', 'pids.max': '384',
                      'cpu.max': '200000 100000'}
            for name, value in values.items():
                (group / name).write_text(value)
            resource_group.verify_job_profile(group, 'edge')
            for name, wrong in [('memory.high', 'max'), ('memory.max', str(4 * GIB)),
                                ('memory.swap.max', 'max'), ('pids.max', 'max'),
                                ('cpu.max', 'max 100000'), ('cpu.max', '400000 100000')]:
                with self.subTest(name=name, wrong=wrong):
                    (group / name).write_text(wrong)
                    with self.assertRaises(RuntimeError):
                        resource_group.verify_job_profile(group, 'edge')
                    (group / name).write_text(values[name])

    def test_profile_is_explicit_and_finite(self):
        self.assertEqual(resource_group.job_profile(['--', 'true']), (None, ['--', 'true']))
        self.assertEqual(resource_group.job_profile(['--job-profile', 'edge', '--', 'true']),
                         ('edge', ['--', 'true']))
        for args in (['--job-profile'], ['--job-profile', 'edge'],
                     ['--job-profile', 'unbounded', '--', 'true']):
            with self.assertRaises(ValueError):
                resource_group.job_profile(args)

    def disconnected_wrapper(self, stop_status):
        """CX: self-observation-fast-02 lost its bus; wrapper exited zero."""
        with tempfile.TemporaryDirectory() as directory:
            root = resource_group.Path(directory)
            stop = ({'side_effect': stop_status} if isinstance(stop_status, OSError)
                    else {'return_value': subprocess.CompletedProcess([], stop_status)})
            with patch.object(resource_group, 'STORAGE', root), \
                 patch.object(resource_group, 'RECEIPTS', root / 'receipts'), \
                 patch.object(resource_group, 'filesystem_type', return_value='ext2/ext3'), \
                 patch.object(resource_group, 'memory_limits', return_value=(5 * GIB, 6 * GIB)), \
                 patch.object(sys, 'argv', ['resource_group.py', '--', 'true']), \
                 patch.object(resource_group.subprocess, 'call', return_value=0) as launch, \
                 patch.object(resource_group.subprocess, 'run', **stop):
                status = resource_group.main()
            self.launched, self.storage = launch.call_args[0][0], root
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

    def test_launch_exports_storage_root_and_host_scaled_limits(self):
        self.disconnected_wrapper(0)
        for flag in ('--property=MemoryHigh=' + str(5 * GIB), '--property=MemoryMax=' + str(6 * GIB),
                     '--property=MemorySwapMax=0', '--setenv=FABRIC_STORAGE_ROOT=' + str(self.storage)):
            self.assertIn(flag, self.launched)

    def test_unset_storage_root_resolves_to_the_xdg_cache_default(self):
        with tempfile.TemporaryDirectory() as directory:
            home = resource_group.Path(directory).resolve()
            self.assertEqual(resource_group.storage_root({'HOME': str(home)}),
                             home / '.cache/fabrico11y')
            self.assertEqual(resource_group.storage_root(
                {'HOME': '/unused', 'XDG_CACHE_HOME': str(home / 'xdg'), 'FABRIC_STORAGE_ROOT': ''}),
                home / 'xdg/fabrico11y')
            self.assertEqual(resource_group.storage_root(
                {'HOME': str(home), 'FABRIC_STORAGE_ROOT': str(home / 'chosen')}), home / 'chosen')

    def test_memory_cap_scales_to_the_host_up_to_the_workstation_ceiling(self):
        for host, high, cap in [(8, 5, 6), (16, 10, 12), (64, 16, 20)]:
            self.assertEqual(resource_group.memory_limits(host * GIB, page=4096),
                             (high * GIB, cap * GIB))
        self.assertEqual(resource_group.memory_limits(64 * GIB, 12 * GIB, 4096), (10 * GIB, 12 * GIB))
        self.assertEqual([value % 65536 for value in resource_group.memory_limits(10 * GIB + 12345, page=65536)], [0, 0])
        with self.assertRaisesRegex(RuntimeError, 'insufficient host RAM'):
            resource_group.memory_limits(GIB, page=4096)

    def test_ram_backed_storage_root_is_refused_before_anything_is_created(self):
        with tempfile.TemporaryDirectory() as directory:
            root = resource_group.Path(directory) / 'new/root'
            for kind in ('tmpfs', 'ramfs', ''):
                with patch.object(resource_group, 'filesystem_type', return_value=kind):
                    with self.assertRaisesRegex(RuntimeError, 'RAM-backed'):
                        resource_group.require_storage(root)
            self.assertFalse(root.parent.exists())

    @unittest.skipUnless(os.path.isdir('/dev/shm') and resource_group.filesystem_type('/dev/shm') == 'tmpfs',
                         'needs a tmpfs /dev/shm')
    def test_real_tmpfs_is_refused(self):
        with self.assertRaisesRegex(RuntimeError, 'RAM-backed'):
            resource_group.require_storage('/dev/shm/fabric-storage-root-test')

    def test_disk_backed_owner_storage_root_is_accepted(self):
        owner = resource_group.Path('/run/media/user/data/FabricO11y')
        self.assertEqual(resource_group.storage_root({'FABRIC_STORAGE_ROOT': str(owner)}), owner)
        with tempfile.TemporaryDirectory() as directory, \
             patch.object(resource_group, 'filesystem_type', return_value='ext2/ext3'):
            root = resource_group.Path(directory) / 'owned'
            self.assertEqual(resource_group.require_storage(root), root)
            self.assertEqual(resource_group.require_storage(root), root)
            with patch.object(resource_group.os, 'getuid', return_value=os.getuid() + 1):
                with self.assertRaisesRegex(RuntimeError, 'owned by the invoking user'):
                    resource_group.require_storage(root)
            (root / 'file').write_text('not a directory')
            with self.assertRaises(OSError):
                resource_group.require_storage(root / 'file')

    def test_temporary_files_and_builds_use_storage_root(self):
        self.assertEqual(resource_group.Path(os.environ['FABRIC_STORAGE_ROOT']), resource_group.STORAGE)
        self.assertEqual(resource_group.require_storage(), resource_group.STORAGE)
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
