"""Focused mocked-effect control for the real native guest_main lifecycle."""
import json
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

import custody_native as native


class Surface:
    logs = list(native.NATIVE_PROFILE['logs'])

    @staticmethod
    def profile(revision, paused):
        return dict(revision=revision, paused=paused, logs=list(Surface.logs), metric_interval_s=15)

    def __init__(self, phase):
        self.pause_revision = 2 if phase == 'drain' else 4
        self.desired = self.profile(self.pause_revision - 1, False)
        self.applied = dict(self.desired)
        self.node_running = True
        self.server_running = True
        self.server_ready = True
        self.highwater = 6
        self.acked = 6
        self.marker_ns = None
        self.inspect_retry = True
        self.inspect_count = 0
        self.records = []

    def response(self, argv, check=True):
        argv = list(argv)
        started_ns = time.time_ns()
        exit_code, stdout, stderr = 0, '', ''
        if argv[:3] == ['fabricctl', 'admin', '/root/accept/admin.conf']:
            if argv[3:5] in (['node', 'pause'], ['node', 'resume']):
                action = argv[4]
                if action == 'resume' and not self.server_ready:
                    raise ValueError('literal restart refusal: resume before listener readiness')
                self.desired = self.profile(self.desired['revision'] + 1, action == 'pause')
                if self.node_running:
                    self.applied = dict(self.desired)
            elif argv[3:5] == ['node', 'list']:
                if self.node_running and self.desired != self.applied:
                    self.applied = dict(self.desired)
                if self.applied['paused']:
                    self.marker_ns = self.marker_ns or time.time_ns()
                stdout = json.dumps({'nodes': [dict(
                    name='spindle-1', status='paused' if self.desired['paused'] else 'active',
                    desired_revision=self.desired['revision'], applied_revision=self.applied['revision'],
                    config_error=None, last_poll_unix_s=1, logs=self.desired['logs'],
                    metric_interval_s=self.desired['metric_interval_s'])]})
        elif argv == ['fabricctl', 'inspect', '/etc/fabrico11y/node.conf']:
            self.inspect_count += 1
            if self.inspect_retry:
                self.inspect_retry = False
                exit_code = 1
                stderr = 'fabricctl: log append or rotation in progress; retry inspection\n'
            else:
                stdout = ('node_id=' + '01' * 16 + ' generation=1 batches=6 metric_points=0 '
                    'log_records=0 gaps=0 otlp_payload_bytes=0 committed_bytes=4096 file_bytes=4096 '
                    'coverage_unknown=false recovery_required=false interrupted_append=false '
                    f'next_sequence={self.highwater + 1} acked_through={self.acked} log_backlog_bytes=0\n')
        elif argv == ['systemctl', 'stop', native.UNITS[0]]:
            self.server_running = False
        elif argv == ['systemctl', 'start', native.UNITS[0]]:
            self.server_running = True
            self.server_ready = False
        elif argv == ['systemctl', 'start', native.UNITS[1]]:
            self.node_running = True
        elif argv == ['systemctl', 'stop', native.UNITS[1]]:
            if self.applied != self.profile(self.pause_revision, True):
                raise ValueError('pre-stop guard: registered pause was not applied')
            if self.acked != self.highwater:
                raise ValueError('pre-stop guard: exact-byte ACK below committed high-water')
            self.node_running = False
        elif argv == ['journalctl', '--no-pager', '-q', '-o', 'json', '-b', '-u', native.UNITS[1]]:
            marker_ns = self.marker_ns or time.time_ns()
            rows = [
                self.event(f'config revision={self.pause_revision} paused=true interval_s=15', marker_ns),
                self.event('batch=6 metrics=0 logs=1 gaps=0 spool_bytes=4096 log_backlog_bytes=0 acked_through=6', marker_ns + 1),
                self.event('delivery sequence=6 sha256=' + 'a' * 64 + ' status=ack committed_through=6 elapsed_us=1', marker_ns + 2),
            ]
            stdout = ''.join(json.dumps(row) + '\n' for row in rows)
        record = dict(argv=argv, exit=exit_code, stdout=stdout, stderr=stderr,
                      started_ns=started_ns, finished_ns=time.time_ns())
        self.records.append(record)
        if check and exit_code:
            raise ValueError('observer command failed: ' + argv[0])
        return record

    @staticmethod
    def event(message, at_ns):
        return dict(_PID='101', _BOOT_ID='aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
            _SYSTEMD_UNIT=native.UNITS[1], _EXE='/usr/bin/fabric-node',
            _SYSTEMD_INVOCATION_ID='1' * 32,
            _SYSTEMD_CGROUP='/system.slice/system-fabrico11y.slice/' + native.UNITS[1],
            _UID='1000', _GID='1000', __REALTIME_TIMESTAMP=str(at_ns // 1000), MESSAGE=message)


class QuiesceGuestMainControl(unittest.TestCase):
    def test_real_guest_main_requires_pause_then_drains_stops_and_captures_final_inspection(self):
        self.exercise('drain')

    def test_reboot_requires_active_revision_three_then_pause_four_and_resume_five(self):
        self.exercise('reboot')

    def test_restart_refusal_never_issues_resume_both_phases(self):
        for phase in ('drain', 'reboot'):
            with self.subTest(phase=phase): self.exercise(phase, restart_refusal=True)

    def exercise(self, phase, *, restart_refusal=False):
        surface = Surface(phase)
        fixture = tempfile.TemporaryDirectory()
        self.addCleanup(fixture.cleanup)
        old_fixture = native.FIXTURE
        native.FIXTURE = Path(fixture.name)
        self.addCleanup(setattr, native, 'FIXTURE', old_fixture)

        class EffectGuest(native.Guest):
            def __init__(self, config, phase):
                self.config, self.phase = config, phase
                self.started = time.monotonic()
                self.deadline = self.started + 170
                self.commands = []
                self.process_observations = []
                self.base = native.FIXTURE / config['binding']['run_id']
                self.output = self.base / phase
                self.output.mkdir(parents=True)
                self.saved = {}

            def command(self, argv, check=True):
                record = surface.response(argv, check)
                self.commands.append(record)
                return record

            def process(self):
                return dict(run_id='mock-run', unit=native.UNITS[1], pid=101,
                    boot_id='aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', start_ns=100, end_ns=0,
                    proc_stat='101 (fabric-node) ' + ' '.join(['S'] + ['0'] * 18 + ['17']),
                    status='Uid: 1000 1000 1000 1000\nGid: 1000 1000 1000 1000\n',
                    exe_sha256='b' * 64, invocation_id='1' * 32,
                    calibration=dict(realtime_ns=1000, boottime_ns=500, clock_ticks=100, start_ticks=17))

            def query(self, name):
                surface.records.append(dict(kind='readiness', server_running=surface.server_running))
                if restart_refusal and not surface.server_ready:
                    record = dict(argv=['fabricctl', 'admin', '/root/accept/admin.conf', 'query', json.dumps(
                        dict(kind='logs', node='spindle-1', from_ns=0, to_ns=9223372036854775807, limit=1000))],
                        exit=2, stdout='', stderr='fabricctl: transport: io: Connection refused\n')
                    self.commands.append(record)
                    surface.records.append(record)
                    raise ValueError('observer command failed: fabricctl')
                surface.server_ready = True
                return [dict(body=native.UNREAD, attributes={'log.file.path': native.SOURCE})]

            def save(self, name, value):
                self.saved[name] = value

            def capture(self, name, process=None):
                surface.records.append(dict(kind='capture', phase=name,
                    stop_observed=not surface.node_running, server_stopped=not surface.server_running))
                return dict(boot_id='aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
                    fixture_paths=(native.SOURCE,), sources=[])

            def finish(self):
                pass

        config = dict(binding=dict(run_id='cutoff-effect-fixture'),
                      boot_before='aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa')
        if phase == 'reboot':
            # Prior checkpoints are only effect inputs here. The independent
            # conservation oracle has separate raw-byte and ACK controls.
            base = native.FIXTURE / config['binding']['run_id']
            for prior, name in (('prepare', 'before'), ('installed', 'installed'), ('drain', 'drain')):
                directory = base / prior
                directory.mkdir(parents=True)
                (directory / (name + '-snapshot.json')).write_text(json.dumps(dict(
                    fixture_paths=[native.SOURCE], boot_id='prior-boot', ack=5)))
            for prior, name in (('prepare', 'baseline.json'), ('drain', 'rows.json'), ('drain', 'ack-events.json')):
                (base / prior / name).write_text('[]')
        clock = [0.0]
        def sleep(seconds): clock[0] += seconds
        with patch.object(native, 'Guest', EffectGuest), patch.object(native.time, 'sleep', sleep), \
                patch.object(native.time, 'monotonic', lambda: clock[0]), \
                patch.object(native.witness, 'conserve') as conservation:
            if restart_refusal:
                with self.assertRaisesRegex(ValueError, 'native drain deadline'):
                    native.guest_main(config, phase)
                self.assertFalse(any(row.get('argv', [])[3:5] == ['node', 'resume'] for row in surface.records))
                self.assertEqual(clock[0], 40)
                return
            native.guest_main(config, phase)
        if phase == 'reboot':
            self.assertTrue(conservation.call_args.kwargs['native_configuration_transition'])
        else:
            conservation.assert_not_called()

        commands = [record for record in surface.records if 'argv' in record]
        node_stop = next(i for i, row in enumerate(commands)
                         if row['argv'] == ['systemctl', 'stop', native.UNITS[1]])
        pause = next(i for i, row in enumerate(commands)
                     if row['argv'][3:5] == ['node', 'pause'])
        resume = next(i for i, row in enumerate(commands)
                      if row['argv'][3:5] == ['node', 'resume'])
        server_stop = next(i for i, row in enumerate(commands)
                           if row['argv'] == ['systemctl', 'stop', native.UNITS[0]])
        self.assertLess(pause, node_stop)
        self.assertLess(node_stop, server_stop)
        self.assertLess(server_stop, resume)
        final_inspect = max(i for i, row in enumerate(commands)
                            if row['argv'] == ['fabricctl', 'inspect', '/etc/fabrico11y/node.conf'])
        capture_at = next(i for i, row in enumerate(surface.records) if row.get('kind') == 'capture')
        final_inspect_at = next(i for i, row in enumerate(surface.records)
                                if row.get('argv') == ['fabricctl', 'inspect', '/etc/fabrico11y/node.conf']
                                and i > capture_at)
        server_start = next(i for i, row in enumerate(commands)
                            if row['argv'] == ['systemctl', 'start', native.UNITS[0]])
        self.assertLess(server_stop, capture_at)
        self.assertLess(capture_at, final_inspect_at)
        self.assertLess(final_inspect, server_start)
        self.assertLess(server_start, resume)
        restart_at = next(i for i, row in enumerate(surface.records)
                          if row.get('argv') == ['systemctl', 'start', native.UNITS[0]])
        resume_at = next(i for i, row in enumerate(surface.records)
                         if row.get('argv', [])[3:5] == ['node', 'resume'])
        self.assertTrue(any(row.get('kind') == 'readiness' and row['server_running']
                           for row in surface.records[restart_at + 1:resume_at]))
        self.assertTrue(surface.records[capture_at]['stop_observed'])
        self.assertTrue(surface.records[capture_at]['server_stopped'])
        self.assertEqual(surface.desired, Surface.profile(surface.pause_revision + 1, False))
        self.assertEqual(surface.applied, Surface.profile(surface.pause_revision, True))
        self.assertEqual(surface.node_running, phase == 'reboot')
        self.assertTrue(surface.server_running)
        self.assertEqual(sum(row['exit'] == 1 for row in commands), 1)
        self.assertGreaterEqual(surface.inspect_count, 4)


if __name__ == '__main__':
    unittest.main()
