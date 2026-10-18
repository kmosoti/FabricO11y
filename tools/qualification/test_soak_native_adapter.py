"""Finite adapter controls; native syscall controls belong to the observer."""
import os
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

from soak_r3 import launch_native, prepare_simulator_output, simulator_command, validate_native_checkpoint


class NativeAdapterControls(unittest.TestCase):
    def test_actual_default_umask_failure_gets_exclusive_private_output(self):
        failure = json.loads((Path(__file__).parent / 'fixtures/soak-witness-output-permissions.json').read_text())
        self.assertEqual(failure['output_mode'], '0755')
        self.assertEqual(failure['helper_stderr'],
                         'spindle_sim: witness parent must be canonical, owned and private\n')
        for mask in (0o022, 0o077):
            with self.subTest(umask=mask), tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
                raw = Path(directory).resolve()
                previous = os.umask(mask)
                try:
                    prepare_simulator_output(raw)
                finally:
                    os.umask(previous)
                info = (raw / 'sim').stat()
                self.assertEqual(info.st_mode & 0o7777, 0o700)
                self.assertEqual(info.st_uid, os.geteuid())
                with self.assertRaises(FileExistsError):
                    prepare_simulator_output(raw)

    def test_output_never_repairs_existing_or_symlinked_destinations(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            raw = Path(directory).resolve()
            target = raw / 'target'
            target.mkdir(mode=0o755)
            (raw / 'sim').symlink_to(target, target_is_directory=True)
            with self.assertRaises(FileExistsError):
                prepare_simulator_output(raw)
            self.assertEqual(target.stat().st_mode & 0o777, 0o755)
            alias = raw / 'alias'
            alias.symlink_to(target, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, 'canonical simulator output parent required'):
                prepare_simulator_output(alias)
            self.assertFalse((target / 'sim').exists())

    def test_restrictive_umask_fails_closed_without_chmod_repair(self):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            raw = Path(directory).resolve()
            previous = os.umask(0o777)
            try:
                with self.assertRaisesRegex(ValueError, 'private owned simulator output required'):
                    prepare_simulator_output(raw)
            finally:
                os.umask(previous)
            self.assertEqual((raw / 'sim').stat().st_mode & 0o7777, 0)
            (raw / 'sim').rmdir()

    def test_actual_failed_command_is_repaired_without_changing_workload(self):
        failure = json.loads((Path(__file__).parent / 'fixtures/soak-witness-output-boundary.json').read_text())
        original = failure['command']
        option = lambda command, name: command[command.index(name) + 1]
        raw = Path(option(original, '--tokens')).parent
        self.assertNotEqual(Path(option(original, '--begin-witness')).parent, Path(option(original, '--out')))
        self.assertEqual(failure['helper_stderr'], 'spindle_sim: begin witness must use the owned output directory\n')
        repaired = simulator_command(18, option(original, '--server-url'),
            Path(option(original, '--ca')).parent, raw, 70)
        self.assertEqual(Path(option(repaired, '--begin-witness')).parent, Path(option(repaired, '--out')))
        expected = list(original)
        expected[expected.index('--begin-witness') + 1] = str(raw / 'sim/begin.json')
        self.assertEqual(repaired, expected)

    def test_stopped_reader_joins_relocated_witness_to_original_raw_path(self):
        from simulator_native_trace import NativeTrace
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root = Path(directory)
            trace = root / 'native-trace'
            trace.mkdir()
            command = simulator_command(18, 'https://localhost', Path('/work'), Path('/original/raw'), 70)
            expected = {'process': {'pid': 7}, 'binary_sha256': '2' * 64, 'begin_sha256': '1' * 64}
            measured = {'native_begin': {'process': {'pid': 7}}, 'begin_sha256': '1' * 64,
                'simulator_command': command, 'checkpoint': {'raw': {'source': '/original/raw'}}}
            record = {'command': command, 'begin_path': '/original/raw/sim/begin.json'}
            path = trace / 'observer-receipt.json'
            path.write_text(json.dumps(record))
            (trace / 'syscalls.raw').write_text('')
            calls = [{'name': 'execve', 'args': repr(['/bootstrap', [], []])[1:-1]},
                {'name': 'execve', 'args': repr([command[0], command, []])[1:-1]}]
            with mock.patch.object(NativeTrace, 'validate_stopped', return_value={'native': 'validated'}) as native, \
                 mock.patch('simulator_native_trace.trace_calls', return_value=calls):
                self.assertEqual(validate_native_checkpoint(root, measured, expected, '2' * 64), {'native': 'validated'})
                native.assert_called_once_with(trace, root / 'raw/sim/begin.json', expected)
                record['begin_path'] = '/original/raw/begin.json'
                path.write_text(json.dumps(record))
                with self.assertRaisesRegex(ValueError, 'native raw command/checkpoint path mismatch'):
                    validate_native_checkpoint(root, measured, expected, '2' * 64)

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
