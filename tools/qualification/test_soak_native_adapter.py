"""Finite adapter controls; native syscall controls belong to the observer."""
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

from soak_r3 import launch_native, validate_native_checkpoint


class NativeAdapterControls(unittest.TestCase):
    def launch(self, root, *, target_affinity=None, wrong_caller=False):
        supervisor = root / 'supervisor'
        supervisor.mkdir()
        observer = mock.Mock()
        observer.start.return_value.target_pid = 7
        clock = SimpleNamespace(launcher_begin=1_000_000_000, call=lambda fn, *args, **kw: fn(*args, **kw))
        affinities = [{0, 1, 2, 3}, {2, 3}, target_affinity or {2, 3}]
        original = Path.read_text
        def read(path, *args, **kwargs):
            if str(path) == '/proc/7/cgroup':
                return '0::' + str(supervisor).removeprefix('/sys/fs/cgroup') + '\n'
            return original(path, *args, **kwargs)
        with mock.patch('cgroups.current', return_value=root if wrong_caller else supervisor), \
             mock.patch('os.sched_getaffinity', side_effect=affinities), \
             mock.patch('os.sched_setaffinity') as setter, mock.patch.object(Path, 'read_text', read):
            try:
                result = launch_native(observer, SimpleNamespace(parent=root), ['/proc/self/fd/99'], 99,
                    root / 'trace', {'strace': {'path': '/pinned/strace', 'sha256': '1' * 64}}, clock)
                self.assertIs(result, observer.start.return_value)
                self.assertEqual(observer.start.call_args.args[0], ['/proc/self/fd/99'])
                self.assertEqual(observer.start.call_args.kwargs['deadline'], 7501)
                return observer
            finally:
                self.assertEqual(setter.call_args.args, (0, {0, 1, 2, 3}))

    def test_direct_held_native_launch_and_affinity_restoration(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            self.launch(Path(directory))

    def test_wrong_target_affinity_and_wrong_caller_fail(self):
        for kwargs in ({'target_affinity': {0, 1}}, {'wrong_caller': True}):
            with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
                with self.assertRaises(ValueError):
                    self.launch(Path(directory), **kwargs)

    def test_other_valid_trace_identity_cannot_replace_consumed_begin(self):
        measurement = {'native_begin': {'process': {'pid': 7}}, 'begin_sha256': '1' * 64}
        for expected in ({'process': {'pid': 8}, 'binary_sha256': '2' * 64, 'begin_sha256': '1' * 64},
                         {'process': {'pid': 7}, 'binary_sha256': '3' * 64, 'begin_sha256': '1' * 64}):
            with self.assertRaises(ValueError):
                validate_native_checkpoint(Path('/not-consumed'), measurement, expected, '2' * 64)


if __name__ == '__main__':
    unittest.main()
