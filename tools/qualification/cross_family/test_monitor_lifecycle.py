"""Independent systemd states and actual failed-command output remain evidence."""
import base64
import hashlib
import json
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import guest
import measure

FIXTURE = Path(__file__).resolve().parents[3] / 'docs/experiments/formal/fixtures/cross-monitor-states.json'


class MonitorState(unittest.TestCase):
    def test_producer_and_outage_require_original_retained_unit_status(self):
        controls = json.loads(FIXTURE.read_text())['controls']
        for unit in ('cross-producer', 'cross-outage'):
            for kind in ('retained', 'ordinary', 'failed'):
                status = controls[kind]['status']
                commands = []
                def script(command, **kwargs):
                    commands.append(command)
                    return subprocess.CompletedProcess(command, 0, status, '')
                value = type('FixtureGuest', (), {'role': 'edge', 'boot_id': 'fixture-boot'})()
                value.script = script
                with tempfile.TemporaryDirectory() as name:
                    root = Path(name)
                    if kind == 'retained':
                        self.assertEqual(measure.monitor_health(value, root, kind, unit)['SubState'], 'exited')
                    else:
                        with self.assertRaises(ValueError):
                            measure.monitor_health(value, root, kind, unit)
                    saved = json.loads((root/('edge-'+unit.removeprefix('cross-')+'-'+kind+'-status.json')).read_text())
                    self.assertEqual(saved['stdout'], status)
                    self.assertEqual(saved['exit'], 0)  # Transport succeeds even for a failed/unloaded body.
                    self.assertIn('systemctl show '+unit+' ', commands[0])

    def test_actual_retained_success_accepts_and_failed_or_unloaded_states_refuse(self):
        controls = json.loads(FIXTURE.read_text())['controls']
        self.assertEqual(measure.require_monitor_health(controls['retained']['status'])['ExecMainStatus'], '0')
        for name in ('ordinary', 'failed'):
            with self.subTest(name=name), self.assertRaises(ValueError):
                measure.require_monitor_health(controls[name]['status'])
        self.assertEqual(controls['failed']['stop_exit'], 0)  # A clean stop cannot certify body success.

    def test_running_monitor_is_healthy_and_partial_or_duplicate_status_refuses(self):
        good = json.loads(FIXTURE.read_text())['controls']['retained']['status']
        running = good.replace('SubState=exited', 'SubState=running').replace('ExecMainCode=1', 'ExecMainCode=0')
        self.assertEqual(measure.require_monitor_health(running)['SubState'], 'running')
        for bad in (good.replace('Result=success\n', ''), good + 'ExecMainStatus=0\n',
                    good.replace('RemainAfterExit=yes', 'RemainAfterExit=no')):
            with self.assertRaises(ValueError):
                measure.require_monitor_health(bad)

    def test_actual_sampler_acknowledges_controlled_stop_with_original_exit_zero(self):
        with tempfile.TemporaryDirectory() as name:
            output = Path(name) / 'samples.jsonl'
            process = subprocess.Popen([sys.executable, '-B', str(Path(__file__).with_name('monitor.py')),
                '--service-cgroup', '/unavailable-monitor-test', '--out', str(output)],
                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 5
                while not output.exists() or not output.stat().st_size:
                    self.assertIsNone(process.poll())
                    if time.monotonic() >= deadline:
                        self.fail('sampler did not arm')
                    time.sleep(.01)
                process.terminate()
                stdout, stderr = process.communicate(timeout=3)
                self.assertEqual(process.returncode, 0, stderr)
                rows = [json.loads(line) for line in output.read_text().splitlines()]
                self.assertTrue(rows)
                self.assertLess(len(rows), 330)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.communicate(timeout=3)

    def test_failure_after_finish_request_is_not_promoted_by_signal_success(self):
        controls = json.loads(FIXTURE.read_text())['controls']
        running = controls['retained']['status'].replace('SubState=exited', 'SubState=running').replace('ExecMainCode=1', 'ExecMainCode=0')
        answers = iter([running, '', controls['failed']['status']])
        value = type('FixtureGuest', (), {'role': 'edge', 'boot_id': 'fixture-boot',
            'script': lambda self, command, **kwargs: subprocess.CompletedProcess(command, 0, next(answers), '')})()
        with tempfile.TemporaryDirectory() as name, self.assertRaises(ValueError):
            measure.finish_monitor(value, Path(name))


class DrainInspection(unittest.TestCase):
    def probe(self, root, code=0, stdout='', stderr=''):
        return type('FixtureGuest', (), {'role': 'edge', 'boot_id': 'fixture-boot',
            'script': lambda self, command, **kwargs:
                subprocess.CompletedProcess(command, code, stdout, stderr)})()

    def test_actual_busy_fixture_is_not_snapshot_and_original_attempt_is_retained(self):
        fixture = json.loads((FIXTURE.parent / 'cross-inspection-retry.json').read_text())
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            value = self.probe(root, fixture['exit'], fixture['stdout'], fixture['stderr'])
            for attempt in range(1, 4):
                self.assertIsNone(measure.inspect(value, '/fixture.conf', root, attempt))
                record = json.loads((root / f'edge-drain-inspection-{attempt}.json').read_text())
                self.assertEqual(record['exit'], 1)
                self.assertEqual(record['stderr'], fixture['stderr'])
                self.assertEqual(record['boot_id'], 'fixture-boot')

    def test_permanent_contention_consumes_existing_drain_deadline(self):
        begin = {'boot_id': 'fixture-boot', 'seconds': 195, 'guard_seconds': 5,
                 'elapsed_seconds': 200, 'monotonic_ns': 0, 'unix_ns': 0}
        class BusyGuest:
            boot_id = 'fixture-boot'
            ticks = 141
            def correlation(self):
                self.ticks += 60
                return {'monotonic_before_ns': self.ticks*10**9, 'unix_ns': 0}
            def script(self, command, **kwargs):
                return subprocess.CompletedProcess(command, 0, json.dumps(begin), '')
        class Observer:
            def start(self, begin):
                pass
        def call(path, *args):
            return {'revision': 1} if path.endswith('/config') else {'nodes': [
                {'name': 'edge', 'applied_revision': 1, 'config_error': None}]}
        with tempfile.TemporaryDirectory() as name, \
                patch.object(measure, 'monitor_health', return_value={'SubState': 'exited'}), \
                patch.object(measure, 'fixed_queries', return_value=[]), \
                patch.object(measure, 'fetch', return_value=[]), \
                patch.object(measure, 'inspect', return_value=None) as inspect, \
                patch.object(measure.time, 'sleep'):
            with self.assertRaisesRegex(RuntimeError, 'registered native/control drain deadline'):
                measure.collect_original(BusyGuest(), BusyGuest(), call, {'log': '/fixture'},
                                         Path(name), 7443, Observer())
            self.assertEqual(inspect.call_count, 2)

    def test_only_exact_retry_diagnostics_with_original_exit_one_are_retried(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            for text in measure.INSPECTION_RETRIES:
                self.assertIsNone(measure.inspect(self.probe(root, 1, '', 'fabricctl: '+text+'\n'),
                                                 '/fixture.conf', root, 1))
            for code, stdout, stderr in ((2, '', 'fabricctl: '+text+'\n'),
                    (1, 'partial', 'fabricctl: '+text+'\n'), (1, '', 'unknown failure\n'),
                    (0, 'recovery_required=true interrupted_append=false next_sequence=3 acked_through=2', ''),
                    (0, 'recovery_required=false interrupted_append=true next_sequence=3 acked_through=2', ''),
                    (0, 'recovery_required=false interrupted_append=false', '')):
                with self.assertRaises((ValueError, KeyError)):
                    measure.inspect(self.probe(root, code, stdout, stderr), '/fixture.conf', root, 1)
            good = 'recovery_required=false interrupted_append=false next_sequence=3 acked_through=2'
            self.assertEqual(measure.inspect(self.probe(root, 0, good), '/fixture.conf', root, 1)['acked_through'], '2')


class ScriptFailureEvidence(unittest.TestCase):
    def make_guest(self, root):
        value = object.__new__(guest.Guest)
        value.role, value.results, value.ssh, value.boot_id = 'edge', root, ['fixture-ssh'], 'fixture-boot'
        return value

    def test_actual_shell_status_and_outputs_preserved_without_stdin_or_overwrite(self):
        text = "secret=fixture-credential-sentinel\nprintf 'original stdout\\n'\nprintf 'original stderr\\n' >&2\nexit 5\n"
        def transport(argv, *, input_text, timeout, check):
            return subprocess.run(['bash', '-s'], input=input_text, text=True,
                                  capture_output=True, timeout=timeout, check=check)
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            value = self.make_guest(root)
            for index in (1, 2):
                with patch.object(guest.Q, 'run', side_effect=transport), \
                     self.assertRaises(subprocess.CalledProcessError) as raised:
                    value.script(text)
                self.assertEqual(raised.exception.returncode, 5)
                path = root / f'edge-command-failure-{index:04}.json'
                evidence = json.loads(path.read_text())
                self.assertEqual(evidence['exit'], 5)
                self.assertEqual(evidence['stdout'], 'original stdout\n')
                self.assertEqual(evidence['stderr'], 'original stderr\n')
                self.assertEqual(evidence['stdin_sha256'], hashlib.sha256(text.encode()).hexdigest())
                self.assertNotIn('fixture-credential-sentinel', path.read_text())
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            self.assertEqual(len(list(root.glob('*-command-failure-*.json'))), 2)

    def test_timeout_retains_unknown_exit_and_exact_binary_partial_output(self):
        error = subprocess.TimeoutExpired(['fixture'], 3, output=b'partial\xff', stderr=b'error\xfe')
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            value = self.make_guest(root)
            with patch.object(guest.Q, 'run', side_effect=error), \
                 self.assertRaises(subprocess.TimeoutExpired) as raised:
                value.script('private fixture input')
            self.assertIs(raised.exception, error)
            evidence = json.loads(next(root.glob('*-command-failure-*.json')).read_text())
            self.assertIsNone(evidence['exit'])
            self.assertEqual(evidence['timeout_s'], 3)
            self.assertEqual(base64.b64decode(evidence['stdout']['data']), error.stdout)
            self.assertEqual(base64.b64decode(evidence['stderr']['data']), error.stderr)


if __name__ == '__main__':
    unittest.main()
