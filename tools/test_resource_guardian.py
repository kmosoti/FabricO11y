"""Finite independent literal controls for the new guardian boundary."""
import copy
import unittest

from resource_guardian import validate_observation
from test_resource_final_group import groups


def fixture():
    workload = groups()
    outer = copy.deepcopy(workload)
    outer['.'].update({'pids.current': '1', 'cgroup.events': 'populated 1\nfrozen 0'})
    outer['control-plane'] = dict(outer['.'], **{'cgroup.procs': '700'})
    outer['workload'] = copy.deepcopy(workload['.'])
    return {'state': 'observed_workload_empty', 'wrapper_wait_exit': 0, 'wrapper_reaped': True,
            'descendants_echild': True, 'wrapper_identity': {'pid': 701, 'start_ticks': 11, 'boot_id': 'native-boot'},
            'guardian_identity': {'pid': 700, 'start_ticks': 10, 'boot_id': 'native-boot'},
            'systemd': {'MainPID': '700', 'ActiveState': 'active', 'SubState': 'running'},
            'activation_monotonic_ns': 1, 'workload_cgroup': '/sys/fs/cgroup/owned.service/workload',
            'command_receipt': {'exit': 0, 'state': 'completed', 'cgroup': '/sys/fs/cgroup/owned.service/workload'},
            'workload': {'groups': workload}, 'outer': {'groups': outer}}


class GuardianControls(unittest.TestCase):
    def test_empty_workload_and_only_guardian_outer(self):
        self.assertTrue(validate_observation(fixture()))

    def test_live_unreaped_wrong_wrapper_and_mainpid(self):
        mutations = [('wrapper_reaped', False), ('descendants_echild', False),
                     ('wrapper_wait_exit', 7), ('activation_monotonic_ns', 0)]
        for key, value in mutations:
            with self.subTest(key=key):
                record = fixture()
                record[key] = value
                with self.assertRaises(ValueError):
                    validate_observation(record)
        for identity, field, value in [('wrapper_identity', 'pid', 700), ('systemd', 'MainPID', '701'),
                                       ('command_receipt', 'cgroup', '/wrong')]:
            record = fixture()
            record[identity][field] = value
            with self.assertRaises(ValueError):
                validate_observation(record)

    def test_every_missing_outer_or_workload_field_refused(self):
        for branch in ('outer', 'workload'):
            for key in groups()['.']:
                with self.subTest(branch=branch, key=key):
                    record = fixture()
                    del record[branch]['groups']['.'][key]
                    with self.assertRaises((ValueError, KeyError)):
                        validate_observation(record)

    def test_dirty_workload_and_escaped_task_refused(self):
        for key, value in [('cgroup.procs', '701'), ('pids.current', '1'), ('cgroup.events', 'populated 1'),
                           ('memory.swap.current', '1'), ('memory.events', 'oom 1\noom_kill 0'),
                           ('memory.max', '999'), ('cpu.max', '10000 100000')]:
            with self.subTest(key=key):
                record = fixture()
                record['workload']['groups']['.'][key] = value
                with self.assertRaises(ValueError):
                    validate_observation(record)
        for name in ('.', 'control-plane', 'unregistered'):
            record = fixture()
            record['outer']['groups'].setdefault(name, dict(record['outer']['groups']['.']))['cgroup.procs'] = '700\n999'
            with self.assertRaises(ValueError):
                validate_observation(record)


if __name__ == '__main__':
    unittest.main()
