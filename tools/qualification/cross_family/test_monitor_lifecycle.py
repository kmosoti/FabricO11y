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
