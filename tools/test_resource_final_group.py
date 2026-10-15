"""R3 final-parent finite controls; native source stays independently observed."""
import copy
import unittest
from pathlib import Path
from unittest import mock

from resource_final_group import options, validate_empty, observe_service


def groups():
    return {'.': {'memory.high': '5368709120', 'memory.max': '6442450944', 'memory.swap.max': '0',
        'pids.max': '1536', 'memory.events': 'oom 0\noom_kill 0', 'memory.peak': '1000',
        'memory.current': '500', 'memory.swap.current': '0', 'pids.current': '0', 'pids.peak': '100',
        'cgroup.events': 'populated 0\nfrozen 0', 'cgroup.procs': '', 'cpu.max': 'max 100000',
        'cpu.stat': 'usage_usec 100', 'io.stat': ''}}


class FinalGroupControls(unittest.TestCase):
    def controller(self, *, receipt=None, states=None, transport_exit=None, stop_exit=0, elapsed=False):
        unit = 'fabric-work-literal'
        child = {'state': 'completed', 'exit': 0, 'cgroup': '/sys/fs/cgroup/' + unit + '.service'}
        if receipt is not None:
            child = receipt
        state = {'ActiveState': 'active', 'SubState': 'exited', 'MainPID': '0',
                 'ActiveEnterTimestampMonotonic': '1000000', 'ControlGroup': '/' + unit + '.service',
                 'ExecMainStatus': '0', 'Result': 'success'}
        transport = mock.Mock()
        transport.poll.return_value = transport_exit
        transport.wait.return_value = 0
        with mock.patch('resource_final_group.subprocess.Popen', return_value=transport), \
             mock.patch('resource_final_group.subprocess.run', return_value=mock.Mock(returncode=stop_exit)), \
             mock.patch('resource_final_group.show', side_effect=states if states else None, return_value=state), \
             mock.patch('resource_final_group.os.open', return_value=77), \
             mock.patch('resource_final_group.os.close'), \
             mock.patch('resource_final_group.snapshot', return_value={'device': 7, 'inode': 9, 'groups': groups()}), \
             mock.patch('resource_final_group.time.monotonic', side_effect=[0, 7500] if elapsed else None, return_value=1):
            return observe_service(['literal-launcher'], Path('/literal/receipt'), unit, ['literal'], 7500,
                                   lambda *_: child)

    def test_controller_requires_actual_inside_exit_and_actual_empty_fields(self):
        status, report = self.controller()
        self.assertEqual(status, 0)
        self.assertEqual(report['state'], 'observed_empty')
        self.assertEqual(report['activation_monotonic_ns'], 1_000_000_000)

    def test_controller_missing_receipt_early_transport_timeout_and_failed_stop(self):
        for kwargs in [{'receipt': {}, 'transport_exit': 0}, {'elapsed': True}, {'stop_exit': 1},
                       {'receipt': {'state': 'completed', 'exit': 0, 'cgroup': '/sys/fs/cgroup/other.service'}}]:
            with self.subTest(kwargs=kwargs):
                _, report = self.controller(**kwargs)
                self.assertEqual(report['state'], 'failed')
                self.assertTrue(report['forced_cleanup'])

    def test_completed_receipt_does_not_prove_wrapper_exited(self):
        live = {'ActiveState': 'active', 'SubState': 'running', 'MainPID': '123',
                'ActiveEnterTimestampMonotonic': '1000000', 'ControlGroup': '/fabric-work-literal.service',
                'ExecMainStatus': '0', 'Result': 'success'}
        _, report = self.controller(states=[live], transport_exit=0)
        self.assertEqual(report['state'], 'failed')

    def test_mode_rejects_unregistered_limits_before_storage_or_launch(self):
        import resource_group
        for arguments in [['--observe-final-group', '--', 'true'],
                          ['--observe-final-group', '--delegate', '--runtime-seconds', '1800', '--', 'true']]:
            with mock.patch('resource_group.sys.argv', ['resource_group.py', *arguments]), \
                 mock.patch('resource_group.require_storage', side_effect=AssertionError('storage touched')):
                with self.assertRaises(ValueError):
                    resource_group.main()

    def test_optional_mode_does_not_change_default_argument_parser(self):
        self.assertEqual(options(['--', 'true']), (False, ['--', 'true']))
        self.assertEqual(options(['--observe-final-group', '--delegate', '--runtime-seconds', '7500', '--', 'true']),
                         (True, ['--delegate', '--runtime-seconds', '7500', '--', 'true']))

    def test_complete_empty_parent_and_subgroup(self):
        observed = groups()
        observed['supervisor'] = dict(observed['.'])
        self.assertTrue(validate_empty(observed))

    def test_literal_missing_field_population_caps_swap_and_oom(self):
        for name, value in [('memory.high', '17179869184'), ('memory.max', '21474836480'),
                            ('memory.swap.max', 'max'), ('pids.max', '4096'),
                            ('memory.swap.current', '1'), ('memory.events', 'oom 0\noom_kill 1'),
                            ('cgroup.events', 'populated 1'), ('cgroup.procs', '123'),
                            ('pids.current', '1'), ('memory.peak', '6442450945')]:
            bad = groups()
            bad['.'][name] = value
            with self.subTest(name=name), self.assertRaises(ValueError):
                validate_empty(bad)
        for name in groups()['.']:
            bad = groups()
            del bad['.'][name]
            with self.subTest(missing=name), self.assertRaises(ValueError):
                validate_empty(bad)
        with self.assertRaises(ValueError):
            validate_empty({})

    def test_empty_parent_cannot_hide_populated_or_incomplete_subgroup(self):
        for child in [{'cgroup.events': 'populated 0'}, dict(groups()['.'], **{'pids.current': '1'})]:
            bad = groups()
            bad['supervisor'] = child
            with self.assertRaises(ValueError):
                validate_empty(bad)


if __name__ == '__main__':
    unittest.main()
